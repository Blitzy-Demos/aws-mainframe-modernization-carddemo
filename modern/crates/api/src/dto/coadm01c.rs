//! The admin menu contract: endpoint 3, `GET /api/v1/menus/admin` (transaction `CA00`,
//! COADM01C, map `COADM1A`; AAP 0.3.4.1).
//!
//! - **Gate.** The handler takes `api::auth::extract::AdminUser`: 401 without a valid token and
//!   403 for a `usr_type` other than `A`, before the service runs (COADM01C-001, n5). COADM01C
//!   tests no user type itself, so the body depends on no identity and on no request field.
//! - **Response.** 200 with the `MenuResponse` of `dto::comen01c`, the schema endpoint 2 also
//!   returns: the six options of `COADM02Y` in table order, `01` (`COUSR00C`) to `06`
//!   (`COTRTUPC`), each admin-only, installed and carrying `This option is not installed ...`,
//!   and the invalid-option text, with no `user_type` and no `admin_only_message`
//!   (COADM01C-005 to COADM01C-014). The menu is a fixed table, not a collection, so the
//!   endpoint takes no `page_size` (Bounded Pagination).
//! - **Client-side keys.** ENTER's option edit, the not-installed and invalid-key messages, the
//!   transfers to the six option screens and F3 back to sign-on all run on the `COADM01` screen
//!   from the response it holds, with no further request (COADM01C-002 to COADM01C-004,
//!   COADM01C-015 to COADM01C-022).
//!
//! # Why this module declares nothing
//!
//! `domain::online::coadm01c::admin_menu` builds the main menu's own
//! `domain::online::comen01c::Menu`, which the `From<Menu>` conversion of `dto::comen01c`
//! already maps, so the handler maps the service result through `MenuResponse::from`. A
//! second conversion from that type would collide with the first, and an admin schema of its
//! own would put a second menu payload type into the `OpenAPI` document and the frontend's
//! `schema.d.ts` (Generated API Types Only). What this module holds is the proof that the shared
//! conversion gives endpoint 3 its body.

#[cfg(test)]
mod tests {
    use domain::online::coadm01c as rules;
    use serde_json::{Value, json};

    use crate::dto::comen01c::MenuResponse;

    /// The not-installed text every admin option carries (COADM01C-014).
    const NOT_INSTALLED: &str = "This option is not installed ...";

    /// One option row of endpoint 3's body as the client receives it: the two digits, the row
    /// text as the 40 bytes of its `OPTNnnn` field, trailing spaces included, the program, and
    /// the flags every admin option has (COADM01C-006 to COADM01C-013).
    fn option(number: &str, name: &str, program: &str) -> Value {
        let text = [number, ". ", name].concat();
        json!({
            "number": number,
            "text": format!("{text:<40}"),
            "program": program,
            "admin_only": true,
            "installed": true,
            "unavailable_message": NOT_INSTALLED,
        })
    }

    // audit: COADM01C-005
    // audit: COADM01C-006
    // audit: COADM01C-007
    // audit: COADM01C-008
    // audit: COADM01C-009
    // audit: COADM01C-010
    // audit: COADM01C-011
    // audit: COADM01C-012
    // audit: COADM01C-013
    // audit: COADM01C-014
    #[test]
    fn endpoint_3_body_is_the_six_option_admin_menu() -> Result<(), serde_json::Error> {
        let body = serde_json::to_value(MenuResponse::from(rules::admin_menu()))?;
        let expected = json!({
            "options": [
                option("01", "User List (Security)", "COUSR00C"),
                option("02", "User Add (Security)", "COUSR01C"),
                option("03", "User Update (Security)", "COUSR02C"),
                option("04", "User Delete (Security)", "COUSR03C"),
                option("05", "Transaction Type List/Update (Db2)", "COTRTLIC"),
                option("06", "Transaction Type Maintenance (Db2)", "COTRTUPC"),
            ],
            "invalid_option_message": "Please enter a valid option number...",
        });
        assert_eq!(body, expected);
        Ok(())
    }
}
