//! axum handlers, one module per resource family; [`router()`] is the only
//! registry.
//!
//! Each module serves the REST resources of one group of `CardDemo` online
//! transactions [README.md:L271-294], or of the rewards system:
//!
//! | Module | Transactions | Resources |
//! |---|---|---|
//! | [`auth`] | CC00 | `/auth/login`, `/auth/logout` |
//! | [`accounts`] | CAVW, CAUP | `/accounts/{id}` |
//! | [`cards`] | CCLI, CCDL, CCUP | `/cards`, `/cards/{id}` |
//! | [`transactions`] | CT00, CT01, CT02 | `/transactions`, `/transactions/{id}` |
//! | [`reports`] | CR00 | `/reports/transactions` |
//! | [`payments`] | CB00 | `/payments` |
//! | [`authorizations`] | CPVS, CPVD | `/authorizations`, `/authorizations/{id}` |
//! | [`admin_users`] | CU00–CU03 | `/admin/users`, `/admin/users/{id}` |
//! | [`admin_transaction_types`] | CTTU, CTLI | `/admin/transaction-types`, `/admin/transaction-types/{id}` |
//! | [`rewards`] | — | `/rewards/balance/{account_id}`, `/rewards/history/{account_id}` |
//! | [`admin_reward_rules`] | — | `/admin/reward-rules`, `/admin/reward-rules/{id}` |
//!
//! The other five online transactions have no route. CM00 and CA00 are menus,
//! replaced by the SPA's sidebar navigation. CP00, CDRD and CDRA are started by
//! MQ triggers, and their replacements are the `RabbitMQ` consumers of
//! `carddemo-messaging`.
//!
//! # One registry
//!
//! The server and the `OpenAPI` document are both built from [`router()`]:
//! `crate::app` serves its axum half, and `crate::openapi::spec` merges its
//! document half. A route therefore cannot be served without being documented,
//! or documented without being served. Handler modules never import from this
//! file, and nothing else registers a route.
//!
//! # Identity
//!
//! Only `POST /auth/login` and `POST /auth/logout` are public. Every other
//! user route takes [`crate::auth::AuthUser`], so a request without a valid
//! bearer token is 401. Every `/admin/**` route, the reward-rule routes
//! included, takes [`crate::auth::AdminUser`], so a valid token whose
//! `usr_type` is not `A` is 403. The acting user is always the verified
//! token's, never a request field.

// The declarations keep rustfmt's default alphabetical order, which the
// `cargo fmt --check` gate enforces; `router()` keeps the transaction-table
// order.
pub mod accounts;
pub mod admin_reward_rules;
pub mod admin_transaction_types;
pub mod admin_users;
pub mod auth;
pub mod authorizations;
pub mod cards;
pub mod payments;
pub mod reports;
pub mod rewards;
pub mod transactions;

use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::state::AppState;

/// Every REST route of the `CardDemo` API: the 19 online transactions served
/// over HTTP and the rewards resources, 19 paths and 30 operations.
///
/// Each `routes!` group holds the operations of exactly one path, because
/// `OpenApiRouter::routes` mounts the group's method router on every path
/// the group documents. Paths use axum 0.8 brace captures (`{id}`), the
/// same text as each handler's `#[utoipa::path(path = ..)]`.
///
/// The router carries no layers and no state: `crate::app` adds the trace and
/// CORS layers and the [`AppState`], and `crate::openapi::spec` only reads the
/// document.
#[must_use]
pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        // CC00 (COSGN00C)
        .routes(routes!(auth::login))
        .routes(routes!(auth::logout))
        // CAVW (COACTVWC), CAUP (COACTUPC)
        .routes(routes!(accounts::get_account, accounts::update_account))
        // CCLI (COCRDLIC), CCDL (COCRDSLC), CCUP (COCRDUPC)
        .routes(routes!(cards::list_cards))
        .routes(routes!(cards::get_card, cards::update_card))
        // CT00 (COTRN00C), CT02 (COTRN02C), CT01 (COTRN01C)
        .routes(routes!(
            transactions::list_transactions,
            transactions::create_transaction
        ))
        .routes(routes!(transactions::get_transaction))
        // CR00 (CORPT00C)
        .routes(routes!(reports::transaction_report))
        // CB00 (COBIL00C)
        .routes(routes!(payments::create_payment))
        // CPVS (COPAUS0C), CPVD (COPAUS1C with COPAUS2C)
        .routes(routes!(authorizations::list_authorizations))
        .routes(routes!(
            authorizations::get_authorization,
            authorizations::toggle_fraud
        ))
        // CU00 (COUSR00C), CU01 (COUSR01C), CU02 (COUSR02C), CU03 (COUSR03C)
        .routes(routes!(admin_users::list_users, admin_users::create_user))
        .routes(routes!(
            admin_users::get_user,
            admin_users::update_user,
            admin_users::delete_user
        ))
        // CTLI (COTRTLIC), CTTU (COTRTUPC)
        .routes(routes!(
            admin_transaction_types::list_tran_types,
            admin_transaction_types::create_tran_type
        ))
        .routes(routes!(
            admin_transaction_types::update_tran_type,
            admin_transaction_types::delete_tran_type
        ))
        // Rewards: balance and accrual history
        .routes(routes!(rewards::get_reward_balance))
        .routes(routes!(rewards::get_reward_history))
        // Rewards: rule management
        .routes(routes!(
            admin_reward_rules::list_reward_rules,
            admin_reward_rules::create_reward_rule
        ))
        .routes(routes!(
            admin_reward_rules::update_reward_rule,
            admin_reward_rules::delete_reward_rule
        ))
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use serde_json::Value;

    use super::router;

    /// Every operation the registry must serve: path, method, `operationId`.
    const OPERATIONS: [(&str, &str, &str); 30] = [
        ("/auth/login", "post", "login"),
        ("/auth/logout", "post", "logout"),
        ("/accounts/{id}", "get", "get_account"),
        ("/accounts/{id}", "put", "update_account"),
        ("/cards", "get", "list_cards"),
        ("/cards/{id}", "get", "get_card"),
        ("/cards/{id}", "put", "update_card"),
        ("/transactions", "get", "list_transactions"),
        ("/transactions", "post", "create_transaction"),
        ("/transactions/{id}", "get", "get_transaction"),
        ("/reports/transactions", "get", "transaction_report"),
        ("/payments", "post", "create_payment"),
        ("/authorizations", "get", "list_authorizations"),
        ("/authorizations/{id}", "get", "get_authorization"),
        ("/authorizations/{id}", "put", "toggle_fraud"),
        ("/admin/users", "get", "list_users"),
        ("/admin/users", "post", "create_user"),
        ("/admin/users/{id}", "get", "get_user"),
        ("/admin/users/{id}", "put", "update_user"),
        ("/admin/users/{id}", "delete", "delete_user"),
        ("/admin/transaction-types", "get", "list_tran_types"),
        ("/admin/transaction-types", "post", "create_tran_type"),
        ("/admin/transaction-types/{id}", "put", "update_tran_type"),
        (
            "/admin/transaction-types/{id}",
            "delete",
            "delete_tran_type",
        ),
        ("/rewards/balance/{account_id}", "get", "get_reward_balance"),
        ("/rewards/history/{account_id}", "get", "get_reward_history"),
        ("/admin/reward-rules", "get", "list_reward_rules"),
        ("/admin/reward-rules", "post", "create_reward_rule"),
        ("/admin/reward-rules/{id}", "put", "update_reward_rule"),
        ("/admin/reward-rules/{id}", "delete", "delete_reward_rule"),
    ];

    const HTTP_METHODS: [&str; 8] = [
        "get", "put", "post", "delete", "options", "head", "patch", "trace",
    ];

    /// The document half of the registry, as JSON.
    fn document() -> Value {
        let (_router, api) = router().split_for_parts();
        serde_json::to_value(api).expect("the document serializes")
    }

    /// Every documented operation: `(path, method)` → the operation object.
    fn documented(document: &Value) -> BTreeMap<(String, String), Value> {
        let paths = document["paths"].as_object().expect("paths is an object");
        paths
            .iter()
            .flat_map(|(path, item)| {
                HTTP_METHODS.iter().filter_map(move |method| {
                    item.get(*method)
                        .map(|operation| ((path.clone(), (*method).to_owned()), operation.clone()))
                })
            })
            .collect()
    }

    fn status_documented(operation: &Value, status: &str) -> bool {
        operation["responses"].get(status).is_some()
    }

    #[test]
    fn registry_serves_exactly_the_19_paths_and_30_operations() {
        let document = document();
        let documented = documented(&document);

        let expected: BTreeMap<(String, String), &str> = OPERATIONS
            .iter()
            .map(|(path, method, id)| (((*path).to_owned(), (*method).to_owned()), *id))
            .collect();
        assert_eq!(expected.len(), 30, "the expected operations are distinct");
        assert_eq!(
            documented.keys().collect::<Vec<_>>(),
            expected.keys().collect::<Vec<_>>(),
            "documented operations"
        );

        let paths: BTreeSet<&str> = OPERATIONS.iter().map(|(path, _, _)| *path).collect();
        assert_eq!(paths.len(), 19);
        assert_eq!(
            document["paths"]
                .as_object()
                .expect("paths is an object")
                .len(),
            19
        );

        for (key, id) in &expected {
            assert_eq!(documented[key]["operationId"], *id, "{key:?}");
        }
        let ids: BTreeSet<&str> = expected.values().copied().collect();
        assert_eq!(ids.len(), 30, "operation IDs are unique");
    }

    #[test]
    fn only_sign_on_and_sign_off_are_public() {
        for ((path, method), operation) in documented(&document()) {
            let security = operation.get("security");
            if path.starts_with("/auth/") {
                assert!(security.is_none(), "{method} {path} is public");
            } else {
                assert_eq!(
                    security,
                    Some(&serde_json::json!([{ "bearer_auth": [] }])),
                    "{method} {path} needs a bearer token"
                );
                assert!(
                    status_documented(&operation, "401"),
                    "{method} {path} documents 401"
                );
            }
        }
    }

    #[test]
    fn admin_routes_and_only_admin_routes_document_403() {
        for ((path, method), operation) in documented(&document()) {
            assert_eq!(
                status_documented(&operation, "403"),
                path.starts_with("/admin/"),
                "{method} {path}"
            );
        }
    }

    #[test]
    fn every_path_uses_axum_brace_captures() {
        let document = document();
        for path in document["paths"]
            .as_object()
            .expect("paths is an object")
            .keys()
        {
            assert!(!path.contains(':'), "{path} has no colon capture");
            assert_eq!(
                path.matches('{').count(),
                path.matches('}').count(),
                "{path}"
            );
        }
    }
}
