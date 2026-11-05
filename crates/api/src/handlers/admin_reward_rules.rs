//! The reward-rule administration: `GET` and `POST /admin/reward-rules`,
//! `PUT` and `DELETE /admin/reward-rules/{id}` over the `reward_rules` table
//! (Request › Rewards system › Admin endpoints, AAP 0.3.5).
//!
//! - Rule management requires `usr_type` `A` (o5): every handler takes
//!   [`AdminUser`], so a request without a valid token is 401 and a `U` token
//!   is 403 with `Admin access required`, before any input is read.
//! - Rewards have no COBOL oracle: the README lists DB2 Rewards only as a
//!   roadmap item [README.md:L357-369], so no parity audit row gates this
//!   module. A rule's codes are the fixture codes, such as type `01` Purchase
//!   [app/data/ASCII/trantype.txt:L1] with category `0001` Regular Sales Draft
//!   [app/data/ASCII/trancatg.txt:L1].
//! - The rule checks (a non-negative rate, the minimum not above the maximum,
//!   `active_to` not before `active_from`) and their messages belong to
//!   `carddemo_domain::rewards::validate_rule`. This module converts the body,
//!   passes it through that check, writes and answers; it repeats none of it.
//! - A delete is soft: it ends the rule's active window at the processing
//!   clock's instant (`CARDDEMO_CLOCK_OVERRIDE` pins it, AAP 0.6.4) and never
//!   removes the row, because reward accruals reference its `rule_id`. Its
//!   answer names the `outcome`, `deleted` or `unchanged`. The delete locks
//!   the rule before it reads that instant, so concurrent deletes of one rule
//!   run one after another and each judges the end the one before committed:
//!   exactly one answers `deleted`, and no later one moves the end earlier.
//! - Every rule answered carries its `status` (`scheduled`, `active` or
//!   `ended`) at the processing clock's instant, which each handler reads once
//!   per request, so the client never judges a rule by its own clock.
//! - Points are only earned. Redemption, tiers and external loyalty
//!   integrations are out of scope, so nothing here touches balances.
//!
//! # Units of work
//!
//! Each request runs in one unit of work. The list reads and rolls back. A
//! write commits only once it has succeeded. Every failure after the unit
//! begins, a failed store call or a missing rule, rolls it back explicitly
//! and awaits the `ROLLBACK` before the outcome is recorded, rather than
//! leaving the rollback to the unit being dropped. A `ROLLBACK` that fails
//! after a failed store call is logged and that call's 500 stands; any other
//! failed `ROLLBACK` answers 500. A failed `COMMIT` consumes the unit. When
//! the server rejects it, `PostgreSQL` has already ended the transaction as a
//! rollback, so on that path, as on every other failure path, nothing of the
//! request is committed. A `COMMIT` that fails otherwise, its connection lost
//! or its answer missing, has an unknown outcome: the write may have
//! committed, so the rules are listed again before it is retried. Unlike the
//! legacy writes, no rule write has to survive a later failure.
//!
//! # Telemetry
//!
//! Each handler runs inside an INFO span `reward_rules` with `transaction`
//! (`REWARD-RULES`), `operation` (`list`, `create`, `update`, `delete`),
//! `user_id` (the admin's token subject), `rule_id` (once known) and
//! `outcome`. The outcome is recorded after the unit of work has committed or
//! rolled back (AAP 0.6.2): `listed`, `created`, `updated`, `deleted` or
//! `unchanged` on success, else the error body's code (`INVALID`,
//! `NOT_FOUND`, `UNEXPECTED`). A request dropped before its answer, its client
//! gone, records `cancelled`: an open unit of work is rolled back as it is
//! dropped, unless its commit had already reached the server.

use std::future::Future;

use axum::Json;
use axum::extract::State;
use carddemo_domain::clock::Clock;
use carddemo_domain::error::DomainError;
use carddemo_domain::messages;
use carddemo_domain::numeric::RuleId;
use carddemo_domain::rules::{PortError, validate_rule};
use carddemo_persistence::repos::SoftDeleteOutcome;
use carddemo_persistence::tx::UnitOfWork;
use tracing::field::Empty;
use tracing::{Instrument, Span};

use crate::auth::AdminUser;
use crate::dto::admin_reward_rules::{
    RewardRuleDeleteOutcome, RewardRuleDeleteResponse, RewardRuleRequest, RewardRuleResponse,
};
use crate::error::{ApiError, ApiJson, ApiPath, Created, ErrorBody};
use crate::state::AppState;

/// The `transaction` field of every span this module opens. Rewards have no
/// CICS transaction ID, so the field names the resource.
const TRANSACTION: &str = "REWARD-RULES";

/// The `outcome` of a list.
const LISTED: &str = "listed";

/// The `outcome` of a stored new rule.
const CREATED: &str = "created";

/// The `outcome` of a replaced rule.
const UPDATED: &str = "updated";

/// The 404 answer for a rule ID that names no rule: code `NOT_FOUND`,
/// [`messages::REWARD_RULE_NOT_FOUND`].
fn rule_not_found() -> ApiError {
    ApiError::from(DomainError::not_found(messages::REWARD_RULE_NOT_FOUND))
}

/// The rule the path segment `{id}` names.
///
/// Rule IDs are `BIGINT` identities, so the segment is read as an `i64`.
///
/// # Errors
///
/// The 404 [`rule_not_found`] answer when `raw` is not a decimal `i64`:
/// such a segment cannot name a stored rule, so it is answered as a missing
/// rule rather than as malformed input.
fn parse_rule_id(raw: &str) -> Result<RuleId, ApiError> {
    raw.parse::<i64>()
        .map(RuleId::from)
        .map_err(|_| rule_not_found())
}

/// The handler span of `operation`, for the request of `admin`.
fn request_span(operation: &'static str, admin: &AdminUser) -> Span {
    tracing::info_span!(
        "reward_rules",
        transaction = TRANSACTION,
        operation,
        user_id = %admin.user_id,
        rule_id = Empty,
        outcome = Empty,
    )
}

/// Records the rule a request concerns on its handler span, the current span
/// inside [`traced`].
fn record_rule_id(rule_id: RuleId) {
    Span::current().record("rule_id", rule_id.get());
}

/// Runs `work` inside `span`, then records its outcome there: the outcome
/// `work` names on success, the error body's code on failure.
///
/// `work` commits its unit of work, or rolls it back and awaits the
/// `ROLLBACK`, before it returns, on success and failure alike, so the
/// outcome is recorded after the commit or the rollback. A request dropped
/// before `work` completes records `cancelled`.
async fn traced<T>(
    span: Span,
    work: impl Future<Output = Result<(T, &'static str), ApiError>>,
) -> Result<T, ApiError> {
    let result = crate::cancellable(span.clone(), work.instrument(span.clone())).await;
    let outcome = match &result {
        Ok((_, outcome)) => outcome,
        Err(error) => error.body().code.as_str(),
    };
    span.record("outcome", outcome);
    result.map(|(value, _)| value)
}

/// Rolls `uow` back after the store call that failed with `error`, then
/// answers with that failure: 500 `Unexpected error`, its detail logged.
///
/// The `ROLLBACK` is awaited, so it has run before [`traced`] records the
/// outcome. A failed `ROLLBACK` is only logged and changes no answer: nothing
/// was committed, and the server discards the transaction when its
/// connection closes.
async fn roll_back<T>(uow: UnitOfWork, error: PortError) -> Result<T, ApiError> {
    let answer = ApiError::from(error);
    if let Err(failure) = uow.rollback().await {
        tracing::warn!(error = %failure, "rollback of the unit of work failed");
    }
    Err(answer)
}

/// `GET /admin/reward-rules`: every reward rule, active, future and
/// soft-deleted alike, in `rule_id` order (Request › Rewards system › Admin
/// endpoints, AAP 0.3.5).
///
/// No rules is 200 with an empty list. A soft-deleted rule shows the
/// `active_to` its delete set. Every rule's `status` is taken at one reading
/// of the processing clock.
///
/// # Errors
///
/// - 401 without a valid bearer token, 403 for a non-admin token.
/// - 500 `Unexpected error` when the read fails or a stored rule does not
///   fit its domain type.
#[utoipa::path(
    get,
    path = "/admin/reward-rules",
    tag = "admin-reward-rules",
    responses(
        (
            status = 200,
            description = "Every reward rule, active and soft-deleted, in `rule_id` order",
            body = Vec<RewardRuleResponse>,
        ),
        (status = 401, description = "No valid bearer token", body = ErrorBody),
        (status = 403, description = "The token is not an admin's", body = ErrorBody),
        (status = 500, description = "The rules could not be read", body = ErrorBody),
    ),
    security(("bearer_auth" = [])),
)]
pub async fn list_reward_rules(
    State(state): State<AppState>,
    admin: AdminUser,
) -> Result<Json<Vec<RewardRuleResponse>>, ApiError> {
    traced(request_span("list", &admin), async move {
        let now = state.clock().now();
        let mut uow = state.db().begin().await?;
        let rules = match uow.store().list_reward_rules().await {
            Ok(rules) => rules,
            Err(error) => return roll_back(uow, error).await,
        };
        uow.rollback().await?;
        let body = rules
            .iter()
            .map(|rule| RewardRuleResponse::at(rule, now))
            .collect();
        Ok((Json(body), LISTED))
    })
    .await
}

/// `POST /admin/reward-rules`: stores a new reward rule and answers 201 with
/// it, under the `rule_id` the database assigned (Request › Rewards system ›
/// Admin endpoints, AAP 0.3.5).
///
/// # Errors
///
/// - 400 `INVALID` for a body that does not deserialize, or a field whose
///   text does not convert to its type, with the rejection text and no
///   `fields`.
/// - 400 `INVALID` from `validate_rule`, with one `fields` entry per failed
///   check: `reward_rate` (`reward_rate must not be negative`),
///   `min_transaction_amount` (`min_transaction_amount must not exceed
///   max_transaction_amount`), `active_to` (`active_to must not precede
///   active_from`). The message is the first failure's.
/// - 401 without a valid bearer token, 403 for a non-admin token.
/// - 500 `Unexpected error` when the insert or the commit fails.
#[utoipa::path(
    post,
    path = "/admin/reward-rules",
    tag = "admin-reward-rules",
    request_body = RewardRuleRequest,
    responses(
        (status = 201, description = "The rule as stored", body = RewardRuleResponse),
        (
            status = 400,
            description = "The body does not convert, or a rule check failed",
            body = ErrorBody,
        ),
        (status = 401, description = "No valid bearer token", body = ErrorBody),
        (status = 403, description = "The token is not an admin's", body = ErrorBody),
        (status = 500, description = "The rule could not be stored", body = ErrorBody),
    ),
    security(("bearer_auth" = [])),
)]
pub async fn create_reward_rule(
    State(state): State<AppState>,
    admin: AdminUser,
    ApiJson(body): ApiJson<RewardRuleRequest>,
) -> Result<Created<RewardRuleResponse>, ApiError> {
    traced(request_span("create", &admin), async move {
        let rule = validate_rule(body.into_draft()?)?;
        let now = state.clock().now();
        let mut uow = state.db().begin().await?;
        let saved = match uow.store().insert_reward_rule(rule).await {
            Ok(saved) => saved,
            Err(error) => return roll_back(uow, error).await,
        };
        uow.commit().await?;
        record_rule_id(saved.rule_id);
        Ok((Created(RewardRuleResponse::at(&saved, now)), CREATED))
    })
    .await
}

/// `PUT /admin/reward-rules/{id}`: replaces every column of the rule but its
/// `rule_id` and answers 200 with the rule as stored (Request › Rewards
/// system › Admin endpoints, AAP 0.3.5).
///
/// The checks run in this order: the rule ID, the body's conversion, then
/// `validate_rule`; only then is the rule written.
///
/// # Errors
///
/// - 404 `NOT_FOUND` (`Reward rule not found`) when `{id}` is not an integer
///   or names no rule.
/// - 400 `INVALID` for a body that does not deserialize or convert, or that
///   fails a rule check, as for `POST /admin/reward-rules`.
/// - 401 without a valid bearer token, 403 for a non-admin token.
/// - 500 `Unexpected error` when the update or the commit fails.
#[utoipa::path(
    put,
    path = "/admin/reward-rules/{id}",
    tag = "admin-reward-rules",
    params(("id" = String, Path, description = "The `rule_id` of the rule to replace")),
    request_body = RewardRuleRequest,
    responses(
        (status = 200, description = "The rule as stored", body = RewardRuleResponse),
        (
            status = 400,
            description = "The body does not convert, or a rule check failed",
            body = ErrorBody,
        ),
        (status = 401, description = "No valid bearer token", body = ErrorBody),
        (status = 403, description = "The token is not an admin's", body = ErrorBody),
        (status = 404, description = "No rule has the ID", body = ErrorBody),
        (status = 500, description = "The rule could not be stored", body = ErrorBody),
    ),
    security(("bearer_auth" = [])),
)]
pub async fn update_reward_rule(
    State(state): State<AppState>,
    admin: AdminUser,
    ApiPath(id): ApiPath<String>,
    ApiJson(body): ApiJson<RewardRuleRequest>,
) -> Result<Json<RewardRuleResponse>, ApiError> {
    traced(request_span("update", &admin), async move {
        let rule_id = parse_rule_id(&id)?;
        record_rule_id(rule_id);
        let rule = validate_rule(body.into_draft()?)?;
        let now = state.clock().now();
        let mut uow = state.db().begin().await?;
        let updated = match uow.store().update_reward_rule(rule_id, rule).await {
            Ok(updated) => updated,
            Err(error) => return roll_back(uow, error).await,
        };
        let Some(saved) = updated else {
            uow.rollback().await?;
            return Err(rule_not_found());
        };
        uow.commit().await?;
        Ok((Json(RewardRuleResponse::at(&saved, now)), UPDATED))
    })
    .await
}

/// `DELETE /admin/reward-rules/{id}`: soft-deletes the rule and answers 200
/// with it and the `outcome` (Request › Rewards system › Admin endpoints,
/// AAP 0.3.5).
///
/// The rule's `active_to` becomes the processing clock's instant when it is
/// absent or later (`outcome` `deleted`); a rule that has already ended is
/// answered unchanged (`outcome` `unchanged`). Either way the rule's `status`
/// at that instant is `ended`. The row is never removed, because accruals
/// reference its `rule_id`.
///
/// # Errors
///
/// - 404 `NOT_FOUND` (`Reward rule not found`) when `{id}` is not an integer
///   or names no rule.
/// - 401 without a valid bearer token, 403 for a non-admin token.
/// - 500 `Unexpected error` when the update, the read or the commit fails.
#[utoipa::path(
    delete,
    path = "/admin/reward-rules/{id}",
    tag = "admin-reward-rules",
    params(("id" = String, Path, description = "The `rule_id` of the rule to soft-delete")),
    responses(
        (
            status = 200,
            description = "The rule after the soft delete, or unchanged when it had already ended, \
                with the outcome",
            body = RewardRuleDeleteResponse,
        ),
        (status = 401, description = "No valid bearer token", body = ErrorBody),
        (status = 403, description = "The token is not an admin's", body = ErrorBody),
        (status = 404, description = "No rule has the ID", body = ErrorBody),
        (status = 500, description = "The rule could not be soft-deleted", body = ErrorBody),
    ),
    security(("bearer_auth" = [])),
)]
pub async fn delete_reward_rule(
    State(state): State<AppState>,
    admin: AdminUser,
    ApiPath(id): ApiPath<String>,
) -> Result<Json<RewardRuleDeleteResponse>, ApiError> {
    traced(request_span("delete", &admin), async move {
        let rule_id = parse_rule_id(&id)?;
        record_rule_id(rule_id);
        let mut uow = state.db().begin().await?;
        // Lock the rule before reading the clock: a concurrent delete of the
        // same rule waits here until the one ahead of it ends, then reads its
        // instant and finds the end that delete committed. An instant read
        // before the wait would be older than that end, move it earlier and
        // answer `deleted` a second time.
        match uow.store().lock_reward_rule(rule_id).await {
            Ok(Some(_)) => {}
            Ok(None) => {
                uow.rollback().await?;
                return Err(rule_not_found());
            }
            Err(error) => return roll_back(uow, error).await,
        }
        let now = state.clock().now();
        let deleted = match uow.store().soft_delete_reward_rule(rule_id, now).await {
            Ok(deleted) => deleted,
            Err(error) => return roll_back(uow, error).await,
        };
        let (rule, outcome) = match deleted {
            SoftDeleteOutcome::Deleted(rule) => (rule, RewardRuleDeleteOutcome::Deleted),
            SoftDeleteOutcome::Unchanged(rule) => (rule, RewardRuleDeleteOutcome::Unchanged),
            SoftDeleteOutcome::NotFound => {
                uow.rollback().await?;
                return Err(rule_not_found());
            }
        };
        uow.commit().await?;
        let answer = RewardRuleDeleteResponse {
            rule: RewardRuleResponse::at(&rule, now),
            outcome,
        };
        Ok((Json(answer), outcome.as_str()))
    })
    .await
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use carddemo_domain::error::{DomainError, ErrorKind};
    use carddemo_domain::messages;
    use carddemo_domain::numeric::RuleId;
    use serde_json::{Value, json};
    use tracing::Span;
    use utoipa::OpenApi;

    use super::{LISTED, parse_rule_id, traced};
    use crate::error::ApiError;

    #[derive(OpenApi)]
    #[openapi(paths(
        super::list_reward_rules,
        super::create_reward_rule,
        super::update_reward_rule,
        super::delete_reward_rule
    ))]
    struct Doc;

    fn document() -> Value {
        serde_json::to_value(Doc::openapi()).expect("the document serializes")
    }

    fn reference(name: &str) -> Value {
        json!(format!("#/components/schemas/{name}"))
    }

    #[test]
    fn rule_ids_are_integers_and_any_other_segment_is_a_missing_rule() {
        for (raw, id) in [
            ("1", 1),
            ("0", 0),
            ("9007199254740993", 9_007_199_254_740_993),
        ] {
            assert_eq!(parse_rule_id(raw).expect("an integer"), RuleId::from(id));
        }
        for raw in ["", "abc", "1.0", "1e3", " 1", "1 ", "99999999999999999999"] {
            let error = parse_rule_id(raw).expect_err("not an i64");
            assert_eq!(error.status(), StatusCode::NOT_FOUND, "{raw:?}");
            assert_eq!(error.body().code, ErrorKind::NotFound.code(), "{raw:?}");
            assert_eq!(error.body().message, messages::REWARD_RULE_NOT_FOUND);
            assert!(error.body().fields.is_empty(), "{raw:?}");
        }
    }

    #[test]
    fn every_operation_is_admin_tagged_secured_and_documents_its_statuses() {
        let doc = document();
        // (path, method, operation ID, documented statuses)
        let cases: [(&str, &str, &str, &[&str]); 4] = [
            (
                "/admin/reward-rules",
                "get",
                "list_reward_rules",
                &["200", "401", "403", "500"],
            ),
            (
                "/admin/reward-rules",
                "post",
                "create_reward_rule",
                &["201", "400", "401", "403", "500"],
            ),
            (
                "/admin/reward-rules/{id}",
                "put",
                "update_reward_rule",
                &["200", "400", "401", "403", "404", "500"],
            ),
            (
                "/admin/reward-rules/{id}",
                "delete",
                "delete_reward_rule",
                &["200", "401", "403", "404", "500"],
            ),
        ];
        for (path, method, operation_id, statuses) in cases {
            let operation = &doc["paths"][path][method];
            assert_eq!(
                operation["operationId"],
                json!(operation_id),
                "{path} {method}"
            );
            assert_eq!(
                operation["tags"],
                json!(["admin-reward-rules"]),
                "{operation_id}"
            );
            assert_eq!(
                operation["security"],
                json!([{ "bearer_auth": [] }]),
                "{operation_id}"
            );
            let responses = operation["responses"].as_object().expect("responses");
            let documented: Vec<&str> = responses.keys().map(String::as_str).collect();
            assert_eq!(documented, statuses, "{operation_id}");
            for status in statuses.iter().filter(|status| !status.starts_with('2')) {
                assert_eq!(
                    responses[*status]["content"]["application/json"]["schema"]["$ref"],
                    reference("ErrorBody"),
                    "{operation_id} {status}"
                );
            }
        }
    }

    #[test]
    fn bodies_and_the_rule_id_parameter_are_documented() {
        let doc = document();
        let rules = &doc["paths"]["/admin/reward-rules"];
        let rule = &doc["paths"]["/admin/reward-rules/{id}"];
        let json_schema = |value: &Value| value["content"]["application/json"]["schema"].clone();

        let list = json_schema(&rules["get"]["responses"]["200"]);
        assert_eq!(list["type"], json!("array"));
        assert_eq!(list["items"]["$ref"], reference("RewardRuleResponse"));
        assert_eq!(
            json_schema(&rules["post"]["responses"]["201"])["$ref"],
            reference("RewardRuleResponse")
        );
        for (method, success, body) in [
            ("put", "200", "RewardRuleResponse"),
            ("delete", "200", "RewardRuleDeleteResponse"),
        ] {
            assert_eq!(
                json_schema(&rule[method]["responses"][success])["$ref"],
                reference(body),
                "{method}"
            );
            let params = rule[method]["parameters"].as_array().expect("parameters");
            assert_eq!(params.len(), 1, "{method}");
            assert_eq!(params[0]["name"], json!("id"), "{method}");
            assert_eq!(params[0]["in"], json!("path"), "{method}");
            assert_eq!(params[0]["required"], json!(true), "{method}");
            assert_eq!(params[0]["schema"]["type"], json!("string"), "{method}");
        }
        for body in [&rules["post"]["requestBody"], &rule["put"]["requestBody"]] {
            assert_eq!(json_schema(body)["$ref"], reference("RewardRuleRequest"));
            assert_eq!(body["required"], json!(true));
        }
        assert!(rules["get"]["requestBody"].is_null());
        assert!(rule["delete"]["requestBody"].is_null());
    }

    #[tokio::test]
    async fn traced_passes_the_value_and_the_error_through_unchanged() {
        let value = traced(Span::none(), async { Ok(("rules", LISTED)) }).await;
        assert_eq!(value.expect("the work succeeded"), "rules");

        let failure = traced(Span::none(), async {
            Err::<((), &'static str), _>(ApiError::from(DomainError::not_found(
                messages::REWARD_RULE_NOT_FOUND,
            )))
        })
        .await
        .expect_err("the work failed");
        assert_eq!(failure.status(), StatusCode::NOT_FOUND);
        assert_eq!(failure.body().message, messages::REWARD_RULE_NOT_FOUND);
    }
}
