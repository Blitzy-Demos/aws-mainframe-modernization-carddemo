//! `Customer`: the customer master record of the VSAM `CUSTDAT` file.
//!
//! One field per elementary item of `CVCUS01Y` `CUSTOMER-RECORD`
//! [app/cpy/CVCUS01Y.cpy:L4-23], in copybook order, each typed by its reviewed
//! section A row of `docs/parity/parity-oracle-audit.md`. The record is pure
//! data with no behaviour of its own; the codec layout, the `customers`
//! repository and the account rules build and read it.

use crate::numeric::{Alnum, CustSsn, CustomerId, FicoScore};

/// `CVCUS01Y` `CUSTOMER-RECORD` [app/cpy/CVCUS01Y.cpy:L4-23], RECLN 500: the
/// record of the VSAM `CUSTDAT` file, keyed by [`Customer::cust_id`].
///
/// Field names are the copybook names lower-snake-cased, prefix and spelling
/// kept; each is the column name of the `customers` table, except the key,
/// whose column is `customer_id`. The trailing `FILLER X(168)`
/// (P-CVCUS01Y-019) [app/cpy/CVCUS01Y.cpy:L23] is not a field: the codec layout
/// keeps its bytes and the table does not store it.
///
/// Text fields hold their stored bytes, trailing spaces included, and dates
/// stay text: `CUST-DOB-YYYY-MM-DD` is edited by the date rules, not parsed
/// here. `Debug` withholds the social security number, as [`CustSsn`] does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Customer {
    /// `CUST-ID` `PIC 9(09)` [app/cpy/CVCUS01Y.cpy:L5], P-CVCUS01Y-001: the
    /// customer number and KSDS prime key, SQL column `customer_id`.
    pub cust_id: CustomerId,
    /// `CUST-FIRST-NAME` `PIC X(25)` [app/cpy/CVCUS01Y.cpy:L6], P-CVCUS01Y-002.
    pub cust_first_name: Alnum<25>,
    /// `CUST-MIDDLE-NAME` `PIC X(25)` [app/cpy/CVCUS01Y.cpy:L7], P-CVCUS01Y-003.
    pub cust_middle_name: Alnum<25>,
    /// `CUST-LAST-NAME` `PIC X(25)` [app/cpy/CVCUS01Y.cpy:L8], P-CVCUS01Y-004.
    pub cust_last_name: Alnum<25>,
    /// `CUST-ADDR-LINE-1` `PIC X(50)` [app/cpy/CVCUS01Y.cpy:L9], P-CVCUS01Y-005.
    pub cust_addr_line_1: Alnum<50>,
    /// `CUST-ADDR-LINE-2` `PIC X(50)` [app/cpy/CVCUS01Y.cpy:L10],
    /// P-CVCUS01Y-006.
    pub cust_addr_line_2: Alnum<50>,
    /// `CUST-ADDR-LINE-3` `PIC X(50)` [app/cpy/CVCUS01Y.cpy:L11],
    /// P-CVCUS01Y-007. The account screens show it as the city.
    pub cust_addr_line_3: Alnum<50>,
    /// `CUST-ADDR-STATE-CD` `PIC X(02)` [app/cpy/CVCUS01Y.cpy:L12],
    /// P-CVCUS01Y-008.
    pub cust_addr_state_cd: Alnum<2>,
    /// `CUST-ADDR-COUNTRY-CD` `PIC X(03)` [app/cpy/CVCUS01Y.cpy:L13],
    /// P-CVCUS01Y-009.
    pub cust_addr_country_cd: Alnum<3>,
    /// `CUST-ADDR-ZIP` `PIC X(10)` [app/cpy/CVCUS01Y.cpy:L14], P-CVCUS01Y-010.
    pub cust_addr_zip: Alnum<10>,
    /// `CUST-PHONE-NUM-1` `PIC X(15)` [app/cpy/CVCUS01Y.cpy:L15],
    /// P-CVCUS01Y-011.
    pub cust_phone_num_1: Alnum<15>,
    /// `CUST-PHONE-NUM-2` `PIC X(15)` [app/cpy/CVCUS01Y.cpy:L16],
    /// P-CVCUS01Y-012.
    pub cust_phone_num_2: Alnum<15>,
    /// `CUST-SSN` `PIC 9(09)` [app/cpy/CVCUS01Y.cpy:L17], P-CVCUS01Y-013,
    /// which the audit row labels `Ssn`.
    pub cust_ssn: CustSsn,
    /// `CUST-GOVT-ISSUED-ID` `PIC X(20)` [app/cpy/CVCUS01Y.cpy:L18],
    /// P-CVCUS01Y-014.
    pub cust_govt_issued_id: Alnum<20>,
    /// `CUST-DOB-YYYY-MM-DD` `PIC X(10)` [app/cpy/CVCUS01Y.cpy:L19],
    /// P-CVCUS01Y-015: the date of birth as stored text.
    pub cust_dob_yyyy_mm_dd: Alnum<10>,
    /// `CUST-EFT-ACCOUNT-ID` `PIC X(10)` [app/cpy/CVCUS01Y.cpy:L20],
    /// P-CVCUS01Y-016.
    pub cust_eft_account_id: Alnum<10>,
    /// `CUST-PRI-CARD-HOLDER-IND` `PIC X(01)` [app/cpy/CVCUS01Y.cpy:L21],
    /// P-CVCUS01Y-017.
    pub cust_pri_card_holder_ind: Alnum<1>,
    /// `CUST-FICO-CREDIT-SCORE` `PIC 9(03)` [app/cpy/CVCUS01Y.cpy:L22],
    /// P-CVCUS01Y-018.
    pub cust_fico_credit_score: FicoScore,
}

#[cfg(test)]
mod tests {
    use super::Customer;
    use crate::numeric::{Alnum, CustSsn, CustomerId, FicoScore};

    /// The first fixture customer [app/data/ASCII/custdata.txt:L1]
    /// (P-CVCUS01Y-021).
    fn first_fixture_customer() -> Customer {
        Customer {
            cust_id: CustomerId::new(1).expect("1 fits 9(09)"),
            cust_first_name: Alnum::move_from("Immanuel"),
            cust_middle_name: Alnum::move_from("Madeline"),
            cust_last_name: Alnum::move_from("Kessler"),
            cust_addr_line_1: Alnum::move_from("618 Deshaun Route"),
            cust_addr_line_2: Alnum::move_from("Apt. 802"),
            cust_addr_line_3: Alnum::move_from("Altenwerthshire"),
            cust_addr_state_cd: Alnum::move_from("NC"),
            cust_addr_country_cd: Alnum::move_from("USA"),
            cust_addr_zip: Alnum::move_from("12546"),
            cust_phone_num_1: Alnum::move_from("(908)119-8310"),
            cust_phone_num_2: Alnum::move_from("(373)693-8684"),
            cust_ssn: CustSsn::new(20_973_888).expect("020973888 fits 9(09)"),
            cust_govt_issued_id: Alnum::move_from("00000000000049368437"),
            cust_dob_yyyy_mm_dd: Alnum::move_from("1961-06-08"),
            cust_eft_account_id: Alnum::move_from("0053581756"),
            cust_pri_card_holder_ind: Alnum::move_from("Y"),
            cust_fico_credit_score: FicoScore::new(274).expect("274 fits 9(03)"),
        }
    }

    #[test]
    fn a_clone_equals_its_original() {
        let customer = first_fixture_customer();
        let copy = customer.clone();
        assert_eq!(copy, customer);
        assert_eq!(copy, first_fixture_customer());
        assert_eq!(copy.cust_id.to_string(), "000000001");
        assert_eq!(copy.cust_last_name.trimmed(), "Kessler");
        assert_eq!(copy.cust_fico_credit_score.get(), 274);
    }

    #[test]
    fn a_changed_fico_score_breaks_equality() {
        let customer = first_fixture_customer();
        let mut rescored = customer.clone();
        rescored.cust_fico_credit_score = FicoScore::new(275).expect("275 fits 9(03)");
        assert_ne!(rescored, customer);
    }

    #[test]
    fn debug_withholds_the_social_security_number() {
        let debug = format!("{:?}", first_fixture_customer());
        assert!(debug.contains("cust_ssn: CustSsn(<redacted>)"), "{debug}");
        assert!(!debug.contains("20973888"), "{debug}");
        assert!(debug.contains("Kessler"), "{debug}");
    }
}
