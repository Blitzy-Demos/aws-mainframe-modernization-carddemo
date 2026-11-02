//! The reward read API (request Rewards system; AAP 0.3.5):
//! `GET /rewards/balance/{account_id}` and `GET /rewards/history/{account_id}`.
//!
//! Rewards are a new feature with no COBOL oracle: the README lists DB2
//! Rewards only as a roadmap item [README.md:L357-369], so no parity audit row
//! gates this module. The account key is the `ACCT-ID PIC 9(11)` of
//! `CVACT01Y` [app/cpy/CVACT01Y.cpy:L5].
//!
//! - Identity (AAP 0.6.1): both routes take [`AuthUser`] and are not
//!   admin-gated. `USRSEC` links a user to no account, so any signed-on user
//!   may read the rewards of any account, as every account screen allows.
//! - Accounts: an account key that does not parse as 1 to 11 digits cannot
//!   name an existing account, so it is answered as an unknown account is:
//!   404 with `Account not found`. An existing account without accruals has
//!   the zero balance and an empty history, both 200.
//! - Points are the decimal digit strings of the DTOs, never JSON numbers:
//!   `9007199254740993` arrives exactly.
//! - Redemption is out of scope (AAP 0.2.5): nothing here writes, and
//!   `total_points_redeemed` is never above 0. Accruals are written only by
//!   `RewardEngine::accrue` on CT02 and POSTTRAN.
//!
//! Each request reads inside one unit of work that is rolled back, since it
//! changes nothing, and records one handler span with `transaction`
//! (`REWARDS`), `user_id`, `account_id` and `outcome` (AAP 0.6.2).

use axum::Json;
use axum::extract::State;
use carddemo_domain::entities::{RewardAccrual, RewardBalance};
use carddemo_domain::error::DomainError;
use carddemo_domain::messages;
use carddemo_domain::numeric::{AccountId, AccrualId};
use carddemo_domain::ports::{AccountReader, PortError};
use tracing::Instrument;

use crate::auth::AuthUser;
use crate::dto::rewards::{RewardAccrualResponse, RewardBalanceResponse, RewardHistoryQuery};
use crate::error::{ApiError, ApiPath, ApiQuery, ErrorBody};
use crate::pagination::{Page, PageSize};
use crate::state::AppState;

/// Accruals on a history page when the request gives no `page_size`. Rewards
/// are new, so no COBOL screen fixes the number.
pub const HISTORY_PAGE_ROWS: u16 = 10;

/// The `transaction` field of the handler spans.
const TRANSACTION: &str = "REWARDS";

/// The `outcome` a handler span records for a request answered 200.
const OUTCOME_OK: &str = "ok";

/// The account `raw` names, read through [`AccountReader`] as the CAVW rule
/// reads it.
///
/// # Errors
///
/// - 404 with [`messages::ACCOUNT_NOT_FOUND`] when `raw` is not 1 to 11 ASCII
///   digits, which names no account, or when no account has the ID
///   ([`PortError::NotFound`]).
/// - 500 with [`messages::UNEXPECTED_ERROR`] on any other read failure.
// Generic over the port, as the rules take their stores, rather than bound to
// `PgStore`: the outcomes are then tested without a database.
async fn resolve_account<A: AccountReader>(
    store: &mut A,
    raw: &str,
) -> Result<AccountId, ApiError> {
    let not_found = || ApiError::from(DomainError::not_found(messages::ACCOUNT_NOT_FOUND));
    let account_id: AccountId = raw.parse().map_err(|_| not_found())?;
    match store.read_account(account_id).await {
        Ok(_) => Ok(account_id),
        Err(PortError::NotFound) => Err(not_found()),
        Err(err) => Err(ApiError::from(err)),
    }
}

/// The balance of the account `raw` names, or its zero balance when it has
/// no `reward_balances` row.
///
/// # Errors
///
/// As [`resolve_account`]; 500 when the unit of work cannot begin or roll
/// back, or the balance read fails.
async fn read_balance(state: &AppState, raw: &str) -> Result<RewardBalance, ApiError> {
    let mut uow = state.db().begin().await?;
    let balance = {
        let mut store = uow.store();
        let account_id = resolve_account(&mut store, raw).await?;
        store
            .reward_balance(account_id)
            .await?
            .unwrap_or_else(|| RewardBalance::zero(account_id))
    };
    uow.rollback().await?;
    Ok(balance)
}

/// One history page of the account `raw` names: up to `rows` accruals newest
/// first, from `start` inclusive.
///
/// # Errors
///
/// As [`resolve_account`]; 500 when the unit of work cannot begin or roll
/// back, or the history read fails.
async fn read_history(
    state: &AppState,
    raw: &str,
    start: Option<AccrualId>,
    rows: u16,
) -> Result<Page<RewardAccrualResponse>, ApiError> {
    let mut uow = state.db().begin().await?;
    let accruals = {
        let mut store = uow.store();
        let account_id = resolve_account(&mut store, raw).await?;
        store.reward_history(account_id, start, rows).await?
    };
    uow.rollback().await?;
    Ok(history_page(&accruals, rows))
}

/// The page of the first `rows` accruals. The row after them, the store's
/// look-ahead read, is not shown: its `accrual_id` is the `next_key`, the
/// inclusive start of the next page. History pages forward only, so
/// `prev_key` is always `null`, and no page carries a message.
fn history_page(accruals: &[RewardAccrual], rows: u16) -> Page<RewardAccrualResponse> {
    let mut items: Vec<RewardAccrualResponse> =
        accruals.iter().map(RewardAccrualResponse::from).collect();
    let shown = usize::from(rows);
    let next_key = if items.len() > shown {
        items
            .split_off(shown)
            .into_iter()
            .next()
            .map(|extra| extra.accrual_id)
    } else {
        None
    };
    Page::new(items, next_key, None, None)
}

/// The `outcome` a handler span records: [`OUTCOME_OK`], or the error body's
/// `code` (`NOT_FOUND`, `INVALID`, `UNEXPECTED`).
fn outcome_of<T>(result: &Result<T, ApiError>) -> &str {
    match result {
        Ok(_) => OUTCOME_OK,
        Err(err) => err.body().code.as_str(),
    }
}

/// The reward balance of one account (request Rewards system; AAP 0.3.5).
///
/// An existing account without accruals answers the zero balance. The points
/// are decimal integer strings.
///
/// # Errors
///
/// - 401 when the request has no valid bearer token ([`AuthUser`]).
/// - 404 with `Account not found` when `account_id` is not 1 to 11 digits or
///   names no account.
/// - 500 with `Unexpected error` when the database fails.
// The explicit summary and description keep this rustdoc, with its links and
// sections, out of the published OpenAPI document.
#[utoipa::path(
    get,
    path = "/rewards/balance/{account_id}",
    tag = "rewards",
    summary = "Reward balance of an account",
    description = "The account's earned, redeemed and available reward points as decimal \
                   integer strings. An existing account without accruals has a zero balance. \
                   Redemption is not offered, so total_points_redeemed is always \"0\".",
    params(
        ("account_id" = String, Path, description = "The account number: 1 to 11 digits."),
    ),
    responses(
        (status = 200, description = "The account's reward balance.", body = RewardBalanceResponse),
        (status = 401, description = "No valid bearer token.", body = ErrorBody),
        (status = 404, description = "The account does not exist.", body = ErrorBody),
        (status = 500, description = "Unexpected error.", body = ErrorBody),
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_reward_balance(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(account_id): ApiPath<String>,
) -> Result<Json<RewardBalanceResponse>, ApiError> {
    let span = tracing::info_span!(
        "get_reward_balance",
        transaction = TRANSACTION,
        user_id = user.user_id.as_str(),
        account_id = account_id.as_str(),
        outcome = tracing::field::Empty,
    );
    let result = read_balance(&state, &account_id)
        .instrument(span.clone())
        .await;
    span.record("outcome", outcome_of(&result));
    result.map(|balance| Json(RewardBalanceResponse::from(&balance)))
}

/// One page of an account's reward history, newest first (request Rewards
/// system; AAP 0.3.5).
///
/// The page holds up to `page_size` accruals (default
/// [`HISTORY_PAGE_ROWS`]) from `start_key` inclusive. `next_key` is the
/// `accrual_id` the next page starts at, `null` on the last page; `prev_key`
/// is always `null`. An account without accruals answers `items: []`.
///
/// # Errors
///
/// - 400 with code `INVALID` when `start_key` is not a decimal accrual ID or
///   `page_size` is not 1 to 100.
/// - 401 when the request has no valid bearer token ([`AuthUser`]).
/// - 404 with `Account not found` when `account_id` is not 1 to 11 digits or
///   names no account.
/// - 500 with `Unexpected error` when the database fails.
// Summary and description are explicit for the same reason as above.
#[utoipa::path(
    get,
    path = "/rewards/history/{account_id}",
    tag = "rewards",
    summary = "Reward accrual history of an account",
    description = "The account's reward accruals, newest first, one page at a time. Pass a \
                   page's next_key as start_key to read the next page; prev_key is always \
                   null. An account without accruals has an empty page.",
    params(
        ("account_id" = String, Path, description = "The account number: 1 to 11 digits."),
        RewardHistoryQuery,
    ),
    responses(
        (status = 200, description = "One page of the account's accruals.", body = Page<RewardAccrualResponse>),
        (status = 400, description = "A malformed start_key or page_size.", body = ErrorBody),
        (status = 401, description = "No valid bearer token.", body = ErrorBody),
        (status = 404, description = "The account does not exist.", body = ErrorBody),
        (status = 500, description = "Unexpected error.", body = ErrorBody),
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_reward_history(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(account_id): ApiPath<String>,
    ApiQuery(query): ApiQuery<RewardHistoryQuery>,
) -> Result<Json<Page<RewardAccrualResponse>>, ApiError> {
    let span = tracing::info_span!(
        "get_reward_history",
        transaction = TRANSACTION,
        user_id = user.user_id.as_str(),
        account_id = account_id.as_str(),
        outcome = tracing::field::Empty,
    );
    let rows = query.page_size.map_or(HISTORY_PAGE_ROWS, PageSize::get);
    let result = async {
        let start = query.start_accrual_id()?;
        read_history(&state, &account_id, start, rows).await
    }
    .instrument(span.clone())
    .await;
    span.record("outcome", outcome_of(&result));
    result.map(Json)
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use carddemo_domain::entities::{Account, RewardAccrual};
    use carddemo_domain::messages;
    use carddemo_domain::numeric::{
        AccountGroupId, AccountId, AccrualId, Alnum, Money, RewardPoints, RuleId, TransactionRowId,
    };
    use carddemo_domain::ports::{AccountReader, PortError, PortResult};
    use chrono::{TimeDelta, TimeZone, Utc};
    use serde_json::{Value, json};
    use utoipa::OpenApi;

    use super::{OUTCOME_OK, history_page, outcome_of, resolve_account};
    use crate::error::ApiError;

    /// An account master that answers every read with one outcome and records
    /// the keys it was asked for.
    struct FakeAccounts {
        outcome: PortResult<()>,
        reads: Vec<AccountId>,
    }

    impl FakeAccounts {
        fn answering(outcome: PortResult<()>) -> Self {
            Self {
                outcome,
                reads: Vec::new(),
            }
        }
    }

    impl AccountReader for FakeAccounts {
        async fn read_account(&mut self, id: AccountId) -> PortResult<Account> {
            self.reads.push(id);
            self.outcome.clone().map(|()| account(id))
        }

        async fn read_account_for_update(&mut self, id: AccountId) -> PortResult<Account> {
            self.read_account(id).await
        }
    }

    fn account_id(value: u64) -> AccountId {
        AccountId::new(value).expect("an 11-digit account")
    }

    fn account(id: AccountId) -> Account {
        Account {
            acct_id: id,
            acct_active_status: Alnum::move_from("Y"),
            acct_curr_bal: Money::ZERO,
            acct_credit_limit: Money::ZERO,
            acct_cash_credit_limit: Money::ZERO,
            acct_open_date: Alnum::move_from("2014-11-20"),
            acct_expiraion_date: Alnum::move_from("2025-05-20"),
            acct_reissue_date: Alnum::move_from("2025-05-20"),
            acct_curr_cyc_credit: Money::ZERO,
            acct_curr_cyc_debit: Money::ZERO,
            acct_addr_zip: Alnum::move_from("A000000000"),
            acct_group_id: AccountGroupId::move_from(""),
        }
    }

    /// Accruals with the given IDs, newest first, one minute apart.
    fn accruals(ids: &[i64]) -> Vec<RewardAccrual> {
        let newest = Utc
            .with_ymd_and_hms(2022, 7, 1, 12, 0, 0)
            .single()
            .expect("a valid UTC instant");
        ids.iter()
            .zip(0_i64..)
            .map(|(&id, age)| RewardAccrual {
                accrual_id: AccrualId::from(id),
                account_id: account_id(1),
                transaction_id: TransactionRowId::from(id),
                rule_id: RuleId::from(1),
                points_earned: RewardPoints::try_from(id).expect("non-negative points"),
                created_at: newest - TimeDelta::minutes(age),
            })
            .collect()
    }

    fn assert_account_not_found(error: &ApiError, context: &str) {
        assert_eq!(error.status(), StatusCode::NOT_FOUND, "{context}");
        assert_eq!(error.body().code, "NOT_FOUND", "{context}");
        assert_eq!(
            error.body().message,
            messages::ACCOUNT_NOT_FOUND,
            "{context}"
        );
        assert!(error.body().fields.is_empty(), "{context}");
    }

    #[tokio::test]
    async fn an_account_key_of_1_to_11_digits_is_read_and_returned() {
        for (raw, id) in [
            ("00000000001", 1),
            ("1", 1),
            ("99999999999", 99_999_999_999),
            ("00000000050", 50),
        ] {
            let mut store = FakeAccounts::answering(Ok(()));
            let resolved = resolve_account(&mut store, raw)
                .await
                .expect("an existing account");
            assert_eq!(resolved, account_id(id), "{raw}");
            assert_eq!(store.reads, vec![account_id(id)], "{raw}");
        }
    }

    #[tokio::test]
    async fn an_unparseable_account_key_is_account_not_found_without_a_read() {
        for raw in [
            "",
            " ",
            "abc",
            "-1",
            "+1",
            " 1",
            "1 ",
            "1.0",
            "100000000000",
            "000000000001",
            "\u{0661}",
        ] {
            let mut store = FakeAccounts::answering(Ok(()));
            let error = resolve_account(&mut store, raw)
                .await
                .expect_err("no account");
            assert_account_not_found(&error, raw);
            assert!(store.reads.is_empty(), "{raw:?} must not reach the store");
        }
    }

    #[tokio::test]
    async fn a_missing_account_is_account_not_found() {
        let mut store = FakeAccounts::answering(Err(PortError::NotFound));
        let error = resolve_account(&mut store, "99999999999")
            .await
            .expect_err("no account");
        assert_account_not_found(&error, "99999999999");
        assert_eq!(store.reads, vec![account_id(99_999_999_999)]);
    }

    #[tokio::test]
    async fn any_other_read_failure_is_unexpected_without_its_detail() {
        for failure in [
            PortError::Unavailable("connection reset while reading accounts".to_owned()),
            PortError::Contention,
            PortError::Duplicate,
            PortError::Restricted,
        ] {
            let mut store = FakeAccounts::answering(Err(failure.clone()));
            let error = resolve_account(&mut store, "00000000001")
                .await
                .expect_err("read failed");
            assert_eq!(
                error.status(),
                StatusCode::INTERNAL_SERVER_ERROR,
                "{failure}"
            );
            assert_eq!(error.body().code, "UNEXPECTED", "{failure}");
            assert_eq!(
                error.body().message,
                messages::UNEXPECTED_ERROR,
                "{failure}"
            );
        }
    }

    /// One paging case: the accrual IDs the store returned, the page rows,
    /// the IDs shown and the next key.
    type PageCase = (
        &'static [i64],
        u16,
        &'static [&'static str],
        Option<&'static str>,
    );

    #[test]
    fn the_look_ahead_row_becomes_next_key_and_is_not_shown() {
        let cases: [PageCase; 6] = [
            (&[], 10, &[], None),
            (&[9, 7, 4], 10, &["9", "7", "4"], None),
            (&[9, 7, 4], 3, &["9", "7", "4"], None),
            (&[9, 7, 4, 2], 3, &["9", "7", "4"], Some("2")),
            (&[9, 7], 1, &["9"], Some("7")),
            (
                &[9_007_199_254_740_993, 5, 3],
                1,
                &["9007199254740993"],
                Some("5"),
            ),
        ];
        for (ids, rows, shown, next_key) in cases {
            let page = history_page(&accruals(ids), rows);
            let shown_ids: Vec<&str> = page
                .items
                .iter()
                .map(|item| item.accrual_id.as_str())
                .collect();
            assert_eq!(shown_ids, shown, "{ids:?} / {rows}");
            assert_eq!(page.next_key.as_deref(), next_key, "{ids:?} / {rows}");
            assert_eq!(page.prev_key, None, "{ids:?} / {rows}");
            assert_eq!(page.message, None, "{ids:?} / {rows}");
        }
    }

    #[test]
    fn an_empty_history_serializes_as_an_empty_page() {
        let page = history_page(&[], super::HISTORY_PAGE_ROWS);
        assert_eq!(
            serde_json::to_value(&page).expect("serializes"),
            json!({"items": [], "next_key": null, "prev_key": null})
        );
    }

    #[test]
    fn the_span_outcome_is_ok_or_the_error_code() {
        assert_eq!(outcome_of(&Ok::<(), ApiError>(())), OUTCOME_OK);
        assert_eq!(outcome_of::<()>(&Err(ApiError::unexpected())), "UNEXPECTED");
        assert_eq!(
            outcome_of::<()>(&Err(ApiError::rejection("start_key: bad".to_owned()))),
            "INVALID"
        );
    }

    #[derive(OpenApi)]
    #[openapi(paths(super::get_reward_balance, super::get_reward_history))]
    struct RewardsDoc;

    fn operation<'a>(doc: &'a Value, path: &str) -> &'a Value {
        &doc["paths"][path]["get"]
    }

    fn names(values: &Value, key: &str) -> Vec<String> {
        values
            .as_array()
            .expect("an array")
            .iter()
            .map(|value| value[key].as_str().expect("a string").to_owned())
            .collect()
    }

    #[test]
    fn openapi_documents_both_operations_as_bearer_protected_reward_reads() {
        let doc = serde_json::to_value(RewardsDoc::openapi()).expect("serializes");
        // (path, operation ID, parameter names, response statuses)
        let cases = [
            (
                "/rewards/balance/{account_id}",
                "get_reward_balance",
                vec!["account_id"],
                vec!["200", "401", "404", "500"],
            ),
            (
                "/rewards/history/{account_id}",
                "get_reward_history",
                vec!["account_id", "start_key", "page_size"],
                vec!["200", "400", "401", "404", "500"],
            ),
        ];
        for (path, operation_id, params, statuses) in cases {
            let op = operation(&doc, path);
            assert_eq!(op["operationId"], json!(operation_id), "{path}");
            assert_eq!(op["tags"], json!(["rewards"]), "{path}");
            assert_eq!(op["security"], json!([{"bearer_auth": []}]), "{path}");
            assert_eq!(names(&op["parameters"], "name"), params, "{path}");
            let account = &op["parameters"][0];
            assert_eq!(account["in"], json!("path"), "{path}");
            assert_eq!(account["required"], json!(true), "{path}");
            assert_eq!(account["schema"]["type"], json!("string"), "{path}");
            let responses: Vec<&str> = op["responses"]
                .as_object()
                .expect("an object")
                .keys()
                .map(String::as_str)
                .collect();
            assert_eq!(responses, statuses, "{path}");
            for status in statuses.iter().filter(|status| **status != "200") {
                assert_eq!(
                    op["responses"][*status]["content"]["application/json"]["schema"]["$ref"],
                    json!("#/components/schemas/ErrorBody"),
                    "{path} {status}"
                );
            }
        }
        let balance_ok = &operation(&doc, "/rewards/balance/{account_id}")["responses"]["200"];
        assert_eq!(
            balance_ok["content"]["application/json"]["schema"]["$ref"],
            json!("#/components/schemas/RewardBalanceResponse")
        );
        for schema in [
            "ErrorBody",
            "RewardBalanceResponse",
            "RewardAccrualResponse",
        ] {
            assert!(
                doc["components"]["schemas"][schema].is_object(),
                "{schema} is registered"
            );
        }
    }
}
