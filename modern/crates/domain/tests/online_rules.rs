//! The domain half of binding g1b: the rows of `modern/audit/online/*.md` whose outcome a pure
//! `domain::online` function decides, proved through the crate's public API, plus the
//! redaction of `domain::sensitive` and the argon2id helpers of `domain::password`.
//!
//! Rows decided only by store effects belong to `crates/api/tests/endpoints_*.rs`, and `client`
//! rows to the frontend tests. A `HALTED(Hn)` path (`modern/audit/HALTS.md`) may run here for
//! coverage, but no value it produces is asserted, and a test that only exercises one carries
//! no audit tag (AAP 0.8.5).
//!
//! Fixtures are read in place from `app/data/ASCII` (Local-Only Runtime). Time comes from a
//! `FixedClock`, and time-derived fields are compared by layout, never by value. Assertions that
//! touch a card number, SSN or password compare raw operands with `assert!(a == b, "<rule>")`,
//! so a failure never prints the protected value (Cardholder Data Redaction).

use std::error::Error;
use std::path::Path;

use chrono::{NaiveDate, NaiveDateTime};
use domain::FixedClock;
use domain::batch::loads::decode_fixture;
use domain::copybook::FixedRecord;

type TestResult = Result<(), Box<dyn Error>>;

/// Every record of the fixture `app/data/ASCII/<name>`, decoded through codec A.
fn fixture<R: FixedRecord>(name: &str) -> Result<Vec<R>, Box<dyn Error>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../app/data/ASCII")
        .join(name);
    let bytes = std::fs::read(path)?;
    Ok(decode_fixture::<R>(&bytes)?
        .into_iter()
        .map(|framed| framed.record)
        .collect())
}

/// The calendar date `year-month-day`.
fn date(year: i32, month: u32, day: u32) -> Result<NaiveDate, Box<dyn Error>> {
    Ok(NaiveDate::from_ymd_opt(year, month, day).ok_or("not a calendar date")?)
}

/// `date` at `hour:minute:second`.
fn at(
    date: NaiveDate,
    hour: u32,
    minute: u32,
    second: u32,
) -> Result<NaiveDateTime, Box<dyn Error>> {
    Ok(date
        .and_hms_opt(hour, minute, second)
        .ok_or("not a time of day")?)
}

/// Whether `text` has the layout `picture`: the same length, a digit wherever the picture has
/// `9`, and the picture's own byte everywhere else. Clock-generated fields are compared this way.
fn has_layout(text: &[u8], picture: &[u8]) -> bool {
    text.len() == picture.len()
        && text.iter().zip(picture).all(|(&byte, &slot)| {
            if slot == b'9' {
                byte.is_ascii_digit()
            } else {
                byte == slot
            }
        })
}

/// The clock every time-dependent rule reads here: 2022-07-18 13:45:07, the `INTCALC` PARM date.
fn clock() -> Result<FixedClock, Box<dyn Error>> {
    Ok(FixedClock::new(at(date(2022, 7, 18)?, 13, 45, 7)?))
}

/// The argon2id hash of the DUSRSECJ password, computed once: argon2 at its default cost is slow
/// in an unoptimised build, so the whole file performs five argon2 operations (this hash, two
/// verifications in `password`, and the successful and the wrong-password sign-ons in `cosgn00c`).
fn demo_hash() -> Result<&'static str, Box<dyn Error>> {
    static HASH: std::sync::OnceLock<Result<String, domain::password::PasswordError>> =
        std::sync::OnceLock::new();
    match HASH.get_or_init(|| domain::password::hash(b"PASSWORD")) {
        Ok(hash) => Ok(hash.as_str()),
        Err(error) => Err(Box::new(*error)),
    }
}

mod password {
    use domain::password::{PasswordError, verify};

    use super::{TestResult, demo_hash};

    // audit: CSUSR01Y-004
    #[test]
    fn the_stored_bytes_verify_and_lower_case_never_does() -> TestResult {
        let hash = demo_hash()?;
        assert!(
            hash.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"),
            "CSUSR01Y-004"
        );
        assert!(!hash.contains("PASSWORD"), "CSUSR01Y-004");
        assert!(verify(b"PASSWORD", hash) == Ok(true), "CSUSR01Y-004");
        // E26: COUSR01C stores the password as typed and sign-on compares it upper-cased, so a
        // lower-case password never verifies against the upper-case hash.
        assert!(verify(b"password", hash) == Ok(false), "CSUSR01Y-004");
        Ok(())
    }

    // audit: CSUSR01Y-004
    #[test]
    fn a_malformed_stored_hash_is_an_error_not_a_panic() {
        for stored in ["", "$", "not-a-phc", "$argon2id$v=19$m=19456,t=2,p=1$"] {
            assert_eq!(
                verify(b"PASSWORD", stored),
                Err(PasswordError::MalformedHash)
            );
        }
        let message = PasswordError::MalformedHash.to_string();
        assert_eq!(message, "stored password hash is not a complete PHC string");
    }
}

mod sensitive {
    use std::fmt::{self, Write};

    use domain::copybook::{PicU, PicX};
    use domain::sensitive::{CardNumber, Cvv, Password, SensitiveError, Ssn};
    use serde::Serialize;

    use super::TestResult;

    /// The first fixture card number, `cardxref.txt:1` and `carddata.txt:1`.
    const CARD: &str = "0500024453765740";

    /// Shows a value through its `Serialize` impl: serde's own `Serializer` for
    /// `fmt::Formatter` writes a serialised string as its text, so no JSON crate is needed.
    /// A hand-written capturing serializer is not used here, because `Serializer` requires
    /// floating-point methods whose parameter types the R1 scan forbids in this tree.
    struct Serialised<'a, T>(&'a T);

    impl<T: Serialize> fmt::Display for Serialised<'_, T> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            self.0.serialize(f)
        }
    }

    fn serialised<T: Serialize>(value: &T) -> Result<String, fmt::Error> {
        let mut out = String::new();
        write!(out, "{}", Serialised(value))?;
        Ok(out)
    }

    /// Autoref probe: `(&Probe(value)).serializes()` resolves to [`ViaSerialize`] when the value
    /// implements `Serialize`, and only otherwise auto-references to [`ViaAnything`].
    struct Probe<T>(T);

    trait ViaSerialize {
        fn serializes(self) -> bool;
    }

    impl<T: Serialize> ViaSerialize for &Probe<T> {
        fn serializes(self) -> bool {
            true
        }
    }

    trait ViaAnything {
        fn serializes(self) -> bool;
    }

    impl<T> ViaAnything for &&Probe<T> {
        fn serializes(self) -> bool {
            false
        }
    }

    // audit: PIC-024
    // audit: CVACT02Y-001
    // audit: CVACT03Y-001
    #[test]
    fn a_card_number_shows_twelve_asterisks_and_its_last_four_digits() -> TestResult {
        let card = CardNumber::parse(CARD)?;
        let redacted = "************5740";
        assert_eq!(redacted.len(), 16);
        for shown in [
            format!("{card:?}"),
            format!("{card}"),
            serialised(&card)?,
            card.redacted(),
        ] {
            assert_eq!(shown, redacted);
            assert!(!shown.contains(CARD), "PIC-024");
        }
        assert_eq!(&card.last4(), b"5740");
        assert_eq!(&card.redacted_bytes(), b"************5740");
        assert!(card.expose().as_bytes() == CARD.as_bytes(), "PIC-024");
        assert_eq!(format!("{card:>20}"), format!("    {redacted}"));
        Ok(())
    }

    // audit: PIC-024
    #[test]
    fn the_disclosed_number_serialises_in_full_and_still_debugs_redacted() -> TestResult {
        let disclosed = CardNumber::parse(CARD)?.disclose();
        assert!(serialised(&disclosed)? == CARD, "PIC-024");
        assert_eq!(format!("{disclosed:?}"), "************5740");
        assert_eq!(format!("{disclosed}"), "************5740");
        Ok(())
    }

    // audit: PIC-024
    // audit: CVCUS01Y-013
    #[test]
    fn an_ssn_shows_its_mask_and_last_four_digits() -> TestResult {
        for (raw, redacted) in [("020973888", "***-**-3888"), ("000000001", "***-**-0001")] {
            let ssn = Ssn::parse(raw)?;
            for shown in [
                format!("{ssn:?}"),
                format!("{ssn}"),
                serialised(&ssn)?,
                ssn.redacted(),
            ] {
                assert_eq!(shown, redacted);
                assert!(!shown.contains(raw), "CVCUS01Y-013");
            }
        }
        let typed = Ssn::new(PicU::new(123_456_789)?);
        assert!(typed.expose().get() == 123_456_789, "PIC-024");
        assert_eq!(&typed.last4(), b"6789");
        Ok(())
    }

    // audit: PIC-024
    // audit: CVACT02Y-003
    // audit: CSUSR01Y-004
    #[test]
    fn a_cvv_and_a_password_show_only_asterisks_and_never_serialise() -> TestResult {
        let cvv = Cvv::new(PicU::new(747)?);
        assert_eq!(format!("{cvv:?}"), "***");
        assert_eq!(format!("{cvv}"), "***");
        assert!(cvv.expose().get() == 747, "CVACT02Y-003");
        let password = Password::from_input("Secret1")?;
        assert_eq!(format!("{password:?}"), "********");
        assert_eq!(format!("{password}"), "********");
        assert!(password.expose() == b"Secret1 ", "CSUSR01Y-004");
        let stored = Password::new(PicX::from_array(*b"PASSWORD"));
        assert!(stored.expose() == b"PASSWORD", "CSUSR01Y-004");
        // Neither type implements `Serialize`; the probe proves it against the redacting ones.
        assert!(!(&Probe(cvv)).serializes());
        assert!(!(&Probe(password)).serializes());
        assert!((&Probe(CardNumber::parse(CARD)?)).serializes());
        assert!((&Probe(Ssn::parse("020973888")?)).serializes());
        Ok(())
    }

    // audit: PIC-024
    #[test]
    fn constructors_reject_bad_input_without_echoing_it() -> TestResult {
        for bad in [
            "",
            "050002445376574",
            "05000244537657400",
            "05000244537657a0",
            "０500024453765740",
        ] {
            let error = CardNumber::parse(bad)
                .err()
                .ok_or("a bad card number parsed")?;
            assert_eq!(error, SensitiveError::NotSixteenDigits);
            // The texts are fixed: neither carries any byte of the rejected input.
            assert_eq!(error.to_string(), "card number must be 16 ASCII digits");
            assert_eq!(format!("{error:?}"), "NotSixteenDigits");
            assert!(CardNumber::try_from(bad).is_err(), "PIC-024");
        }
        for bad in ["", "02097388", "0209738880", "02097388x"] {
            let error = Ssn::parse(bad).err().ok_or("a bad SSN parsed")?;
            assert_eq!(error, SensitiveError::NotNineDigits);
            assert_eq!(error.to_string(), "SSN must be 9 ASCII digits");
            assert_eq!(format!("{error:?}"), "NotNineDigits");
            assert!(Ssn::try_from(bad).is_err(), "PIC-024");
        }
        let too_long = Password::from_input("PASSWORD9").err();
        assert_eq!(too_long, Some(SensitiveError::TooLong { max: 8 }));
        let text = SensitiveError::TooLong { max: 8 }.to_string();
        assert_eq!(text, "value must be at most 8 bytes");
        assert!(!text.contains("PASSWORD"), "PIC-024");
        Ok(())
    }
}

mod cosgn00c {
    use domain::copybook::PicX;
    use domain::online::cosgn00c::{
        FailureClass, NextRoute, SignonFailure, SignonField, StoredUser, UserLookup, UserType,
        decide, edit,
    };

    use super::{TestResult, demo_hash};

    // audit: COSGN00C-007
    #[test]
    fn a_blank_user_id_is_reported_first() {
        for (user_id, password) in [("", "PASSWORD"), ("        ", ""), ("", "")] {
            assert_eq!(
                edit(user_id, password).err(),
                Some(SignonFailure::UserIdBlank)
            );
        }
        let failure = SignonFailure::UserIdBlank;
        assert_eq!(failure.message(), "Please enter User ID ...");
        assert_eq!(failure.field(), SignonField::UserId);
        assert_eq!(failure.field().name(), "user_id");
        assert_eq!(failure.class(), FailureClass::Input);
        assert_eq!(failure.class().http_status(), 400);
    }

    // audit: COSGN00C-008
    #[test]
    fn a_blank_password_is_reported_after_the_user_id() {
        for password in ["", "        "] {
            assert_eq!(
                edit("ADMIN001", password).err(),
                Some(SignonFailure::PasswordBlank)
            );
        }
        let failure = SignonFailure::PasswordBlank;
        assert_eq!(failure.message(), "Please enter Password ...");
        assert_eq!(failure.field().name(), "password");
        assert_eq!(failure.class().http_status(), 400);
    }

    // audit: COSGN00C-003
    // audit: COSGN00C-009
    #[test]
    fn inputs_are_upper_cased_8_byte_fields() -> TestResult {
        let key = edit("user0001", "password")?;
        assert_eq!(key.user_id().as_bytes(), b"USER0001");
        assert!(key.password().expose() == b"PASSWORD", "COSGN00C-009");
        let key = edit(" adm", "pw")?;
        assert_eq!(key.user_id().as_bytes(), b" ADM    ");
        assert!(key.password().expose() == b"PW      ", "COSGN00C-003");
        for (user_id, password, field) in [
            ("ADMIN0011", "PASSWORD", SignonField::UserId),
            ("ADMIN001", "PASSWORD1", SignonField::Password),
        ] {
            let failure = edit(user_id, password).err();
            assert_eq!(failure, Some(SignonFailure::Rejected(field)));
        }
        Ok(())
    }

    // audit: COSGN00C-013
    // audit: COSGN00C-014
    // audit: COSGN00C-015
    // audit: COSGN00C-009
    #[test]
    fn an_admin_signs_on_upper_cased_and_goes_to_the_admin_menu() -> TestResult {
        let hash = demo_hash()?;
        let key = edit("admin001", "password")?;
        let lookup = UserLookup::Found(StoredUser::new(UserType::ADMIN, hash));
        let signed_on = decide(&key, lookup).into_result()?;
        assert_eq!(signed_on.user_id.as_bytes(), b"ADMIN001");
        assert_eq!(signed_on.user_type, UserType::ADMIN);
        assert_eq!(signed_on.next_route, NextRoute::Admin);
        assert_eq!(signed_on.next_route.path(), "/admin");
        assert!(
            !format!("{signed_on:?}").contains("PASSWORD"),
            "COSGN00C-013"
        );
        Ok(())
    }

    // audit: COSGN00C-015
    // audit: COSGN00C-016
    #[test]
    fn only_type_a_routes_to_the_admin_menu() {
        let code = |byte: u8| UserType::from_code(PicX::from_array([byte]));
        assert!(UserType::ADMIN.is_admin() && !UserType::ADMIN.is_user());
        assert!(UserType::USER.is_user() && !UserType::USER.is_admin());
        assert_eq!(NextRoute::for_user_type(&UserType::ADMIN).path(), "/admin");
        for byte in [b'U', b' ', b'a', b'X'] {
            let user_type = code(byte);
            assert!(!user_type.is_admin(), "COSGN00C-016");
            assert_eq!(NextRoute::for_user_type(&user_type), NextRoute::Menu);
            assert_eq!(user_type.code().as_bytes(), &[byte]);
        }
        assert_eq!(NextRoute::Menu.path(), "/menu");
    }

    // audit: COSGN00C-018
    // audit: COSGN00C-019
    // audit: COSGN00C-020
    #[test]
    fn failed_sign_ons_carry_their_messages_fields_and_statuses() -> TestResult {
        let found = |hash: &'static str| UserLookup::Found(StoredUser::new(UserType::USER, hash));
        for (rule, password, lookup, expected, message, field, status) in [
            // `PASSWOR` is padded to `PASSWOR `, which differs from the stored `PASSWORD` only in
            // its eighth byte, so a trimmed or prefix comparison would wrongly sign on.
            (
                "COSGN00C-018",
                "PASSWOR",
                found(demo_hash()?),
                SignonFailure::WrongPassword,
                "Wrong Password. Try again ...",
                SignonField::Password,
                401,
            ),
            (
                "COSGN00C-019",
                "PASSWORD",
                UserLookup::NotFound,
                SignonFailure::UserNotFound,
                "User not found. Try again ...",
                SignonField::UserId,
                401,
            ),
            (
                "COSGN00C-020",
                "PASSWORD",
                UserLookup::ReadError,
                SignonFailure::UnableToVerify,
                "Unable to verify the User ...",
                SignonField::UserId,
                500,
            ),
            // A stored hash that is no PHC string cannot be verified (X-COSGN00C-2), so no argon2
            // work is done.
            (
                "COSGN00C-020",
                "PASSWORD",
                found("not-a-phc-string"),
                SignonFailure::UnableToVerify,
                "Unable to verify the User ...",
                SignonField::UserId,
                500,
            ),
        ] {
            let key = edit("USER0001", password)?;
            let failure = decide(&key, lookup)
                .into_result()
                .err()
                .ok_or_else(|| format!("{rule}: a failed sign-on signed on"))?;
            assert_eq!(failure, expected, "{rule}");
            assert_eq!(failure.message(), message, "{rule}");
            assert_eq!(failure.to_string(), message, "{rule}");
            assert_eq!(failure.field(), field, "{rule}");
            assert_eq!(failure.class().http_status(), status, "{rule}");
        }
        Ok(())
    }
}

mod comen01c {
    use domain::copybook::PicX;
    use domain::online::comen01c::{
        ADMIN_ONLY, Access, INVALID_OPTION, Installation, MAIN_MENU_OPTIONS, OPTION_COUNT,
        coming_soon_message, main_menu, not_installed_message, unavailable_message,
    };
    use domain::online::cosgn00c::UserType;

    const OPTIONS: [(&str, &str); 11] = [
        ("Account View", "COACTVWC"),
        ("Account Update", "COACTUPC"),
        ("Credit Card List", "COCRDLIC"),
        ("Credit Card View", "COCRDSLC"),
        ("Credit Card Update", "COCRDUPC"),
        ("Transaction List", "COTRN00C"),
        ("Transaction View", "COTRN01C"),
        ("Transaction Add", "COTRN02C"),
        ("Transaction Reports", "CORPT00C"),
        ("Bill Payment", "COBIL00C"),
        ("Pending Authorization View", "COPAUS0C"),
    ];

    // audit: COMEN01C-020
    // audit: COMEN01C-021
    #[test]
    fn the_eleven_options_are_the_copybook_table_in_order() {
        assert_eq!(usize::from(OPTION_COUNT), MAIN_MENU_OPTIONS.len());
        for ((option, (name, program)), number) in
            MAIN_MENU_OPTIONS.iter().zip(OPTIONS).zip(1_u64..)
        {
            assert_eq!(option.number().get(), number);
            assert_eq!(option.name().to_text(), name);
            assert_eq!(option.name().as_bytes().len(), 35);
            assert_eq!(option.program().to_text(), program);
            assert_eq!(option.access(), Access::AnyUser);
            assert!(!option.access().is_admin_only(), "COMEN01C-020");
            let text = format!("{number:02}. {name:<35} ");
            assert_eq!(option.display_text().as_bytes(), text.as_bytes());
        }
        assert_eq!(Access::from_code(b'A'), Access::AdminOnly);
        assert_eq!(Access::from_code(b'U'), Access::AnyUser);
    }

    // audit: COMEN01C-004
    // audit: COMEN01C-011
    // audit: COMEN01C-013
    // audit: COMEN01C-015
    // audit: COMEN01C-017
    #[test]
    fn the_menu_carries_its_flags_and_texts_for_every_user_type() {
        let for_user = main_menu(&UserType::USER);
        let for_admin = main_menu(&UserType::ADMIN);
        assert_eq!(for_user.entries(), for_admin.entries());
        assert_eq!(for_user.user_type(), Some(&UserType::USER));
        assert_eq!(for_admin.user_type(), Some(&UserType::ADMIN));
        assert_eq!(
            for_user.invalid_option(),
            "Please enter a valid option number..."
        );
        assert_eq!(INVALID_OPTION.len(), 37);
        assert_eq!(
            for_user.admin_only(),
            Some("No access - Admin Only option... ")
        );
        assert_eq!(ADMIN_ONLY.len(), 33);
        assert_eq!(for_user.entries().len(), 11);
        let (last, rest) = for_user.entries().split_last().unzip();
        let last = last.map(|entry| (entry.option().installation(), entry.unavailable_message()));
        assert_eq!(
            last,
            Some((
                Installation::Installed,
                Some("This option Pending Authorization View is not installed...")
            ))
        );
        for entry in rest.unwrap_or_default() {
            assert_eq!(entry.option().installation(), Installation::Installed);
            assert!(entry.option().installation().is_installed(), "COMEN01C-013");
            assert_eq!(entry.unavailable_message(), None);
            assert_eq!(unavailable_message(entry.option()), None);
        }
    }

    // audit: COMEN01C-015
    // audit: COMEN01C-017
    #[test]
    fn unavailable_texts_cut_the_name_as_the_strings_do() {
        let eleventh = &MAIN_MENU_OPTIONS[10];
        assert_eq!(
            not_installed_message(eleventh),
            "This option Pending Authorization View is not installed..."
        );
        assert_eq!(not_installed_message(eleventh).len(), 58);
        // The coming-soon text keeps the name up to its first space and has no blank before
        // `is`; no shipped option reaches it.
        assert_eq!(
            coming_soon_message(eleventh),
            "This option Pendingis coming soon ..."
        );
        assert!(!Installation::ComingSoon.is_installed(), "COMEN01C-017");
        assert!(!Installation::NotInstalled.is_installed(), "COMEN01C-015");
        let blank = PicX::<35>::spaces();
        assert_eq!(blank.as_bytes().len(), 35);
    }
}

mod coadm01c {
    use domain::online::coadm01c::{
        ADMIN_MENU_OPTIONS, ADMIN_OPTION_COUNT, INVALID_OPTION, admin_menu, not_installed_message,
    };
    use domain::online::comen01c::{Access, Installation};

    const OPTIONS: [(&str, &str); 6] = [
        ("User List (Security)", "COUSR00C"),
        ("User Add (Security)", "COUSR01C"),
        ("User Update (Security)", "COUSR02C"),
        ("User Delete (Security)", "COUSR03C"),
        ("Transaction Type List/Update (Db2)", "COTRTLIC"),
        ("Transaction Type Maintenance (Db2)", "COTRTUPC"),
    ];

    // audit: COADM01C-005
    // audit: COADM01C-006
    // audit: COADM01C-007
    // audit: COADM01C-008
    // audit: COADM01C-009
    // audit: COADM01C-010
    // audit: COADM01C-011
    #[test]
    fn the_six_options_are_the_copybook_table_in_order() {
        assert_eq!(usize::from(ADMIN_OPTION_COUNT), ADMIN_MENU_OPTIONS.len());
        let menu = admin_menu();
        assert_eq!(menu.entries().len(), 6);
        for ((entry, (name, program)), number) in menu.entries().iter().zip(OPTIONS).zip(1_u64..) {
            let option = entry.option();
            assert_eq!(option.number().get(), number);
            assert_eq!(option.name().to_text(), name);
            assert_eq!(option.program().to_text(), program);
            let text = format!("{number:02}. {name:<35} ");
            assert_eq!(option.display_text().as_bytes(), text.as_bytes());
        }
    }

    // audit: COADM01C-012
    // audit: COADM01C-013
    // audit: COADM01C-014
    #[test]
    fn every_option_is_admin_only_and_installed_with_the_fixed_texts() {
        let menu = admin_menu();
        assert_eq!(menu.user_type(), None);
        assert_eq!(menu.admin_only(), None);
        assert_eq!(
            menu.invalid_option(),
            "Please enter a valid option number..."
        );
        assert_eq!(INVALID_OPTION, menu.invalid_option());
        for entry in menu.entries() {
            assert_eq!(entry.option().access(), Access::AdminOnly);
            assert_eq!(entry.option().installation(), Installation::Installed);
            assert_eq!(
                entry.unavailable_message(),
                Some("This option is not installed ...")
            );
            assert_eq!(
                not_installed_message(entry.option()),
                "This option is not installed ..."
            );
        }
    }
}

mod coactvwc {
    use domain::copybook::PicU;
    use domain::online::coactvwc::{
        FailureClass, Lookup, ViewFailure, ViewField, edit_account, view,
    };
    use domain::records::{AccountRecord, CardXrefRecord, CustomerRecord};

    use super::{TestResult, date, fixture};

    const FILTER_MESSAGE: &str = "Account Filter must  be a non-zero 11 digit number";

    /// Account 1 of `acctdata.txt:1`, customer 1 of `custdata.txt:1` and the cross-reference
    /// that joins them, `cardxref.txt:49`.
    fn records()
    -> Result<(AccountRecord, CustomerRecord, CardXrefRecord), Box<dyn std::error::Error>> {
        let account = fixture::<AccountRecord>("acctdata.txt")?
            .into_iter()
            .find(|record| record.id.get() == 1)
            .ok_or("no account 1")?;
        let customer = fixture::<CustomerRecord>("custdata.txt")?
            .into_iter()
            .find(|record| record.id.get() == 1)
            .ok_or("no customer 1")?;
        let xref = fixture::<CardXrefRecord>("cardxref.txt")?
            .into_iter()
            .find(|record| record.acct_id.get() == 1)
            .ok_or("no cross-reference for account 1")?;
        Ok((account, customer, xref))
    }

    fn failed<T>(outcome: Result<T, ViewFailure>) -> Result<ViewFailure, &'static str> {
        outcome.err().ok_or("expected a failure")
    }

    // audit: COACTVWC-029
    // audit: COACTVWC-030
    // audit: COACTVWC-031
    // audit: COACTVWC-032
    #[test]
    fn the_account_edit_gives_its_messages_and_the_key() -> TestResult {
        for raw in ["", "           ", "*", "*          "] {
            let failure = failed(edit_account(raw))?;
            assert_eq!(failure.message(), "No input received");
            assert_eq!(failure.field(), Some(ViewField::AcctId));
            assert_eq!(failure.field().map(ViewField::name), Some("acct_id"));
            assert_eq!(failure.class().http_status(), 400);
        }
        for raw in [
            "1",
            "00000000000",
            "0000000001a",
            " 0000000001",
            "*0000000001",
        ] {
            let failure = failed(edit_account(raw))?;
            assert_eq!(failure.message(), FILTER_MESSAGE);
            assert_eq!(failure.class(), FailureClass::Input);
            assert!(failure.account().is_none() && failure.customer().is_none());
        }
        assert_eq!(edit_account("00000000001")?.get(), 1);
        assert_eq!(edit_account("99999999999")?.get(), 99_999_999_999);
        Ok(())
    }

    // audit: COACTVWC-008
    // audit: COACTVWC-010
    // audit: COACTVWC-011
    // audit: COACTVWC-012
    // audit: COACTVWC-013
    // audit: COACTVWC-014
    // audit: COACTVWC-015
    // audit: COACTVWC-033
    // audit: COACTVWC-034
    #[test]
    fn the_fixture_account_shows_its_edited_amounts_and_typed_dates() -> TestResult {
        let (account, customer, xref) = records()?;
        let shown = view(
            PicU::new(1)?,
            Lookup::Found(&xref),
            Lookup::Found(&account),
            Lookup::Found(&customer),
        )?;
        assert_eq!(shown.acct_id.get(), 1);
        let block = &shown.account;
        assert_eq!(block.active_status.as_bytes(), b"Y");
        assert_eq!(block.current_balance, "+        194.00");
        assert_eq!(block.credit_limit, "+      2,020.00");
        assert_eq!(block.cash_credit_limit, "+      1,020.00");
        assert_eq!(block.current_cycle_credit, "+           .00");
        assert_eq!(block.current_cycle_debit, "+           .00");
        assert_eq!(block.current_balance.len(), 15);
        assert_eq!(block.open_date, Some(date(2014, 11, 20)?));
        assert_eq!(block.expiration_date, Some(date(2025, 5, 20)?));
        assert_eq!(block.reissue_date, Some(date(2025, 5, 20)?));
        assert!(block.group_id.is_spaces(), "COACTVWC-015");
        let rendered = format!("{shown:?}");
        assert!(!rendered.contains("0500024453765740"), "COACTVWC-034");
        assert!(!rendered.contains("************"), "COACTVWC-034");
        Ok(())
    }

    // audit: COACTVWC-016
    // audit: COACTVWC-017
    // audit: COACTVWC-018
    // audit: COACTVWC-019
    // audit: COACTVWC-020
    // audit: COACTVWC-021
    // audit: COACTVWC-022
    // audit: COACTVWC-023
    // audit: COACTVWC-024
    // audit: COACTVWC-025
    // audit: COACTVWC-026
    #[test]
    fn the_fixture_customer_shows_its_fields_with_the_ssn_redacted() -> TestResult {
        let (account, customer, xref) = records()?;
        let shown = view(
            PicU::new(1)?,
            Lookup::Found(&xref),
            Lookup::Found(&account),
            Lookup::Found(&customer),
        )?;
        let block = &shown.customer;
        assert_eq!(block.customer_id.get(), 1);
        assert_eq!(block.ssn.to_string(), "***-**-3888");
        assert!(!format!("{shown:?}").contains("020973888"), "COACTVWC-017");
        assert_eq!(block.fico_score.get(), 274);
        assert_eq!(block.date_of_birth, Some(date(1961, 6, 8)?));
        assert_eq!(block.first_name.to_text(), "Immanuel");
        assert_eq!(block.middle_name.to_text(), "Madeline");
        assert_eq!(block.last_name.to_text(), "Kessler");
        assert_eq!(block.address_line_1, customer.addr_line_1);
        assert_eq!(block.address_line_2, customer.addr_line_2);
        assert_eq!(block.city.to_text(), "Altenwerthshire");
        assert_eq!(block.state.as_bytes(), b"NC");
        assert_eq!(block.country.as_bytes(), b"USA");
        assert_eq!(block.zip.as_bytes(), b"12546");
        assert_eq!(block.phone_1.as_bytes(), b"(908)119-8310");
        assert_eq!(block.phone_2.as_bytes(), b"(373)693-8684");
        assert_eq!(block.government_id, customer.govt_issued_id);
        assert_eq!(block.eft_account_id.as_bytes(), b"0053581756");
        assert_eq!(block.primary_card_holder.as_bytes(), b"Y");
        Ok(())
    }

    // audit: COACTVWC-035
    // audit: COACTVWC-037
    #[test]
    fn a_failed_cross_reference_stops_the_chain() -> TestResult {
        let (account, customer, _) = records()?;
        let key = PicU::new(1)?;
        // HALTED(H4): the CICS codes at positions 56-65 and 72-75 are not asserted.
        let missing = failed(view(
            key,
            Lookup::NotFound,
            Lookup::Found(&account),
            Lookup::Found(&customer),
        ))?;
        assert_eq!(
            missing.message().get(..55),
            Some("Account:00000000001 not found in Cross ref file.  Resp:")
        );
        assert_eq!(missing.message().get(65..71), Some(" Reas:"));
        assert_eq!(missing.class(), FailureClass::NotFound);
        assert_eq!(missing.class().http_status(), 404);
        assert_eq!(missing.field(), Some(ViewField::AcctId));
        assert!(missing.account().is_none() && missing.customer().is_none());
        // HALTED(H4): the CICS codes at positions 49-58 and 66-75 are not asserted.
        let broken = failed(view(key, Lookup::Error, Lookup::NotFound, Lookup::NotFound))?;
        assert_eq!(
            broken.message().get(..48),
            Some("File Error: READ     on CXACAIX   returned RESP ")
        );
        assert_eq!(broken.class().http_status(), 500);
        assert_eq!(broken.field(), Some(ViewField::AcctId));
        Ok(())
    }

    // audit: COACTVWC-040
    // audit: COACTVWC-044
    // audit: COACTVWC-046
    #[test]
    fn account_and_customer_read_failures_keep_the_blocks_the_reads_found() -> TestResult {
        let (account, customer, xref) = records()?;
        let key = PicU::new(1)?;
        let no_account = failed(view(
            key,
            Lookup::Found(&xref),
            Lookup::NotFound,
            Lookup::Found(&customer),
        ))?;
        assert_eq!(
            no_account.message().get(..55),
            Some("Account:00000000001 not found in Acct Master file.Resp:")
        );
        assert_eq!(no_account.class().http_status(), 404);
        assert_eq!(no_account.field(), Some(ViewField::AcctId));
        assert!(no_account.customer().is_some(), "COACTVWC-040");
        let no_customer = failed(view(
            key,
            Lookup::Found(&xref),
            Lookup::Found(&account),
            Lookup::NotFound,
        ))?;
        assert_eq!(
            no_customer.message().get(..52),
            Some("CustId:000000001 not found in customer master.Resp: ")
        );
        assert_eq!(no_customer.message().get(62..68), Some(" REAS:"));
        assert_eq!(no_customer.class().http_status(), 404);
        assert_eq!(no_customer.field(), None);
        let block = no_customer
            .account()
            .ok_or("COACTVWC-044: the account block")?;
        assert_eq!(block.current_balance, "+        194.00");
        assert!(no_customer.customer().is_none(), "COACTVWC-044");
        let broken = failed(view(
            key,
            Lookup::Found(&xref),
            Lookup::Found(&account),
            Lookup::Error,
        ))?;
        assert_eq!(
            broken.message().get(..48),
            Some("File Error: READ     on CUSTDAT   returned RESP ")
        );
        assert_eq!(broken.class().http_status(), 500);
        assert_eq!(broken.field(), None);
        assert!(broken.account().is_some(), "COACTVWC-046");
        Ok(())
    }
}

mod coactupc {
    use std::error::Error;

    use domain::copybook::{PicDate, PicU, PicX};
    use domain::money::{Decimal, Money};
    use domain::online::coactupc::{
        AccountUpdateInput, AccountUpdateView, Aid, ChangeCheck, CustomerRewrite, DateParts,
        Decision, FailureClass, Fetched, Lookup, PendingSave, PhoneParts, Snapshot, SsnParts,
        UpdateFailure, UpdateField, UpdateOutcome, UpdateState, account_key, check_change_in_rec,
        compare_old_new, decide_with, edit_with, fetch,
    };
    use domain::online::csutldtc::{DateMask, LeFeedback, LeResult};
    use domain::records::{AccountRecord, CardXrefRecord, CustomerRecord};
    use domain::sensitive::{CardNumber, Ssn};

    use super::{TestResult, clock, date};

    type Outcome<T> = Result<T, Box<dyn Error>>;

    const NO_CHANGE: &str = "No change detected with respect to values fetched.";

    fn money(cents: i64) -> Outcome<Money<10>> {
        Ok(Money::new(Decimal::new(cents, 2))?)
    }

    fn pic_date(year: i32, month: u32, day: u32) -> Outcome<PicDate> {
        Ok(PicDate::new(Some(date(year, month, day)?))?)
    }

    fn text<const N: usize>(value: &str) -> Outcome<PicX<N>> {
        Ok(PicX::from_text(value)?)
    }

    fn account() -> Outcome<AccountRecord> {
        Ok(AccountRecord {
            id: PicU::new(1)?,
            active_status: text("Y")?,
            curr_bal: money(19_400)?,
            credit_limit: money(202_000)?,
            cash_credit_limit: money(102_000)?,
            open_date: pic_date(2014, 11, 20)?,
            expiraion_date: pic_date(2025, 5, 20)?,
            reissue_date: pic_date(2025, 5, 20)?,
            curr_cyc_credit: money(0)?,
            curr_cyc_debit: money(-5_025)?,
            addr_zip: text("A000000000")?,
            group_id: text("GOLD")?,
            filler: PicX::spaces(),
        })
    }

    fn customer() -> Outcome<CustomerRecord> {
        Ok(CustomerRecord {
            id: PicU::new(1)?,
            first_name: text("Immanuel")?,
            middle_name: text("Madeline")?,
            last_name: text("Kessler")?,
            addr_line_1: text("618 Deshaun Route")?,
            addr_line_2: text("Apt. 802")?,
            addr_line_3: text("Altenwerthshire")?,
            addr_state_cd: text("CA")?,
            addr_country_cd: text("USA")?,
            addr_zip: text("90210")?,
            phone_num_1: text("(201)555-1234")?,
            phone_num_2: text("(202)555-9876")?,
            ssn: Ssn::new(PicU::new(123_456_789)?),
            govt_issued_id: text("000000000004936843")?,
            dob_yyyy_mm_dd: pic_date(1961, 6, 8)?,
            eft_account_id: text("0053581756")?,
            pri_card_holder_ind: text("Y")?,
            fico_credit_score: PicU::new(750)?,
            filler: PicX::spaces(),
        })
    }

    fn xref() -> Outcome<CardXrefRecord> {
        Ok(CardXrefRecord {
            card_num: CardNumber::parse("0500024453765740")?,
            cust_id: PicU::new(1)?,
            acct_id: PicU::new(1)?,
            filler: PicX::spaces(),
        })
    }

    fn shown_view() -> Outcome<AccountUpdateView> {
        let (account, customer, xref) = (account()?, customer()?, xref()?);
        Ok(fetch(
            "00000000001",
            Lookup::Found(&xref),
            Lookup::Found(&account),
            Lookup::Found(&customer),
        )?)
    }

    fn accept_all(text: &str, mask: DateMask) -> LeResult {
        LeResult::new(text, mask, LeFeedback::Valid)
    }

    /// The screen fields as the operator sends them back, owned so that a test can change one.
    #[derive(Clone)]
    struct Form {
        texts: [String; 22],
        dates: [[String; 3]; 4],
        phones: [[String; 3]; 2],
        ssn: Option<[String; 3]>,
    }

    // Positions of the single-text fields in `Form::texts`.
    const ACCOUNT: usize = 0;
    const STATUS: usize = 1;
    const CREDIT: usize = 2;
    const CASH: usize = 3;
    const BALANCE: usize = 4;
    const CYCLE_CREDIT: usize = 5;
    const GROUP: usize = 6;
    const CYCLE_DEBIT: usize = 7;
    const CUSTOMER: usize = 8;
    const FICO: usize = 9;
    const FIRST: usize = 10;
    const MIDDLE: usize = 11;
    const LAST: usize = 12;
    const LINE_1: usize = 13;
    const STATE: usize = 14;
    const LINE_2: usize = 15;
    const ZIP: usize = 16;
    const CITY: usize = 17;
    const COUNTRY: usize = 18;
    const GOVERNMENT: usize = 19;
    const EFT: usize = 20;
    const PRIMARY: usize = 21;

    fn owned<const N: usize>(values: [&str; N]) -> [String; N] {
        values.map(str::to_owned)
    }

    fn date_parts([year, month, day]: &[String; 3]) -> DateParts<'_> {
        DateParts { year, month, day }
    }

    fn phone_parts([area, prefix, line]: &[String; 3]) -> PhoneParts<'_> {
        PhoneParts { area, prefix, line }
    }

    impl Form {
        /// The fields of [`shown_view`], unchanged.
        fn shown() -> Outcome<Self> {
            let amounts = shown_view()?.amounts;
            let mut texts = owned([
                "00000000001",
                "Y",
                "",
                "",
                "",
                "",
                "GOLD",
                "",
                "000000001",
                "750",
                "Immanuel",
                "Madeline",
                "Kessler",
                "618 Deshaun Route",
                "CA",
                "Apt. 802",
                "90210",
                "Altenwerthshire",
                "USA",
                "000000000004936843",
                "0053581756",
                "Y",
            ]);
            texts[CREDIT] = amounts.credit_limit;
            texts[CASH] = amounts.cash_credit_limit;
            texts[BALANCE] = amounts.current_balance;
            texts[CYCLE_CREDIT] = amounts.current_cycle_credit;
            texts[CYCLE_DEBIT] = amounts.current_cycle_debit;
            Ok(Self {
                texts,
                dates: [
                    owned(["2014", "11", "20"]),
                    owned(["2025", "05", "20"]),
                    owned(["2025", "05", "20"]),
                    owned(["1961", "06", "08"]),
                ],
                phones: [owned(["201", "555", "1234"]), owned(["202", "555", "9876"])],
                ssn: None,
            })
        }

        /// The shown fields with the last name changed.
        fn changed() -> Outcome<Self> {
            let mut form = Self::shown()?;
            "Kessling".clone_into(&mut form.texts[LAST]);
            Ok(form)
        }

        fn input(&self) -> AccountUpdateInput<'_> {
            let t = |index: usize| self.texts.get(index).map_or("", String::as_str);
            let [open, expiry, reissue, dob] = &self.dates;
            let [phone_1, phone_2] = &self.phones;
            AccountUpdateInput {
                account_id: t(ACCOUNT),
                active_status: t(STATUS),
                open_date: date_parts(open),
                credit_limit: t(CREDIT),
                expiry_date: date_parts(expiry),
                cash_credit_limit: t(CASH),
                reissue_date: date_parts(reissue),
                current_balance: t(BALANCE),
                current_cycle_credit: t(CYCLE_CREDIT),
                group_id: t(GROUP),
                current_cycle_debit: t(CYCLE_DEBIT),
                customer_id: t(CUSTOMER),
                ssn: self.ssn.as_ref().map(|[part1, part2, part3]| SsnParts {
                    part1,
                    part2,
                    part3,
                }),
                dob: date_parts(dob),
                fico_score: t(FICO),
                first_name: t(FIRST),
                middle_name: t(MIDDLE),
                last_name: t(LAST),
                address_line_1: t(LINE_1),
                state: t(STATE),
                address_line_2: t(LINE_2),
                zip: t(ZIP),
                city: t(CITY),
                country: t(COUNTRY),
                phone_1: phone_parts(phone_1),
                government_id: t(GOVERNMENT),
                phone_2: phone_parts(phone_2),
                eft_account_id: t(EFT),
                primary_card_holder: t(PRIMARY),
            }
        }
    }

    type Shown = (String, Option<UpdateField>, u16);

    fn shown(failure: &UpdateFailure) -> Shown {
        (
            failure.message().to_owned(),
            failure.field(),
            failure.class().http_status(),
        )
    }

    fn input_error(message: &str, field: UpdateField) -> Shown {
        (message.to_owned(), Some(field), 400)
    }

    fn snapshot(state: UpdateState) -> Outcome<Snapshot> {
        Ok(Snapshot::from_records(&account()?, &customer()?).with_state(state))
    }

    fn fetched() -> Outcome<Fetched> {
        Ok(Fetched::new(&customer()?))
    }

    fn decided(
        aid: Aid,
        form: &Form,
        state: UpdateState,
    ) -> Outcome<Result<Decision, UpdateFailure>> {
        let snapshot = snapshot(state)?;
        Ok(decide_with(
            aid,
            &form.input(),
            &snapshot,
            &fetched()?,
            &clock()?,
            accept_all,
        ))
    }

    fn edit_failure(form: &Form) -> Outcome<Shown> {
        let outcome = edit_with(&form.input(), &fetched()?, &clock()?, accept_all);
        let failure = outcome.err().ok_or("the edits passed")?;
        assert_eq!(failure.state(), UpdateState::ChangesNotOk);
        Ok(shown(&failure))
    }

    fn pending(form: &Form) -> Outcome<PendingSave> {
        match decided(Aid::Pf5, form, UpdateState::ChangesOkNotConfirmed)? {
            Ok(Decision::Save(save)) => Ok(*save),
            Ok(Decision::Respond(outcome)) => Err(format!("responded {outcome:?}").into()),
            Err(failure) => Err(failure.into()),
        }
    }

    // audit: COACTUPC-045
    // audit: COACTUPC-046
    // audit: COACTUPC-047
    #[test]
    fn the_account_key_edit_gives_its_messages() -> TestResult {
        for blank in ["", "           ", "*          "] {
            let failure = account_key(blank).err().ok_or("a blank key passed")?;
            assert_eq!(
                shown(&failure),
                input_error("No input received", UpdateField::AccountId)
            );
            assert_eq!(failure.state(), UpdateState::NotFetched);
            assert_eq!(failure.field().map(UpdateField::map_name), Some("ACCTSID"));
        }
        for bad in ["123", "0000000000A", "00000000000", "-0000000001"] {
            let failure = account_key(bad).err().ok_or("a bad key passed")?;
            let message = "Account Number if supplied must be a 11 digit Non-Zero Number";
            assert_eq!(
                shown(&failure),
                input_error(message, UpdateField::AccountId)
            );
            assert_eq!(failure.class(), FailureClass::Input);
        }
        assert_eq!(account_key("00000000001")?.get(), 1);
        Ok(())
    }

    // audit: COACTUPC-068
    // audit: COACTUPC-074
    // audit: COACTUPC-075
    // audit: COACTUPC-076
    // audit: COACTUPC-077
    // audit: COACTUPC-079
    // audit: COACTUPC-100
    #[test]
    fn the_fetch_shows_the_stored_values_with_the_ssn_redacted() -> TestResult {
        let view = shown_view()?;
        assert_eq!(view.account_id.get(), 1);
        assert_eq!(view.active_status.as_bytes(), b"Y");
        assert_eq!(view.amounts.current_balance, "+        194.00");
        assert_eq!(view.amounts.credit_limit, "+      2,020.00");
        assert_eq!(view.amounts.cash_credit_limit, "+      1,020.00");
        assert_eq!(view.amounts.current_cycle_credit, "+           .00");
        assert_eq!(view.amounts.current_cycle_debit, "-         50.25");
        assert_eq!(view.open_date, Some(date(2014, 11, 20)?));
        assert_eq!(view.expiry_date, Some(date(2025, 5, 20)?));
        assert_eq!(view.reissue_date, Some(date(2025, 5, 20)?));
        assert_eq!(view.dob, Some(date(1961, 6, 8)?));
        assert_eq!(view.customer_id.get(), 1);
        assert_eq!(view.fico_score.get(), 750);
        assert_eq!(view.ssn.to_string(), "***-**-6789");
        assert!(!format!("{view:?}").contains("123456789"), "COACTUPC-076");
        assert_eq!(view.zip.as_bytes(), b"90210");
        assert_eq!(view.phone_1.area.as_bytes(), b"201");
        assert_eq!(view.phone_1.prefix.as_bytes(), b"555");
        assert_eq!(view.phone_1.line.as_bytes(), b"1234");
        assert_eq!(view.phone_2.line.as_bytes(), b"9876");
        assert_eq!(
            view.info_message(),
            "Update account details presented above."
        );
        assert_eq!(AccountUpdateView::STATE, UpdateState::ShowDetails);
        assert_eq!(view.snapshot.fields().ssn_last4, "6789");
        Ok(())
    }

    // audit: COACTUPC-085
    // audit: COACTUPC-087
    // audit: COACTUPC-091
    // audit: COACTUPC-095
    // audit: COACTUPC-097
    // audit: COACTUPC-099
    #[test]
    fn the_fetch_reports_each_failed_read() -> TestResult {
        let (account, customer, xref) = (account()?, customer()?, xref()?);
        let key = "00000000001";
        let failed = |outcome: Result<AccountUpdateView, UpdateFailure>| {
            outcome.err().ok_or("expected a failure")
        };
        // HALTED(H4): the CICS codes inside each message are not asserted.
        let missing = failed(fetch(
            key,
            Lookup::NotFound,
            Lookup::Found(&account),
            Lookup::Found(&customer),
        ))?;
        assert_eq!(
            missing.message().get(..55),
            Some("Account:00000000001 not found in Cross ref file.  Resp:")
        );
        assert_eq!(missing.message().get(65..71), Some(" Reas:"));
        assert_eq!(
            (missing.class(), missing.field()),
            (FailureClass::NotFound, Some(UpdateField::AccountId))
        );
        assert_eq!(missing.state(), UpdateState::NotFetched);
        let broken = failed(fetch(
            key,
            Lookup::Failed,
            Lookup::NotReached,
            Lookup::NotReached,
        ))?;
        assert_eq!(
            broken.message().get(..48),
            Some("File Error: READ     on CXACAIX   returned RESP ")
        );
        assert_eq!(broken.message().get(58..64), Some(",RESP2"));
        assert_eq!(broken.class().http_status(), 500);
        let no_account = failed(fetch(
            key,
            Lookup::Found(&xref),
            Lookup::NotFound,
            Lookup::Found(&customer),
        ))?;
        assert_eq!(
            no_account.message().get(..55),
            Some("Account:00000000001 not found in Acct Master file.Resp:")
        );
        assert_eq!(no_account.state(), UpdateState::ShowDetails);
        assert_eq!(no_account.field(), Some(UpdateField::AccountId));
        assert!(no_account.details().is_some(), "COACTUPC-099");
        let no_customer = failed(fetch(
            key,
            Lookup::Found(&xref),
            Lookup::Found(&account),
            Lookup::NotFound,
        ))?;
        assert_eq!(
            no_customer.message().get(..52),
            Some("CustId:000000001 not found in customer master.Resp: ")
        );
        assert_eq!(
            (no_customer.class().http_status(), no_customer.field()),
            (404, None)
        );
        assert_eq!(no_customer.state(), UpdateState::NotFetched);
        let customer_error = failed(fetch(
            key,
            Lookup::Found(&xref),
            Lookup::Found(&account),
            Lookup::Failed,
        ))?;
        assert_eq!(
            customer_error.message().get(..48),
            Some("File Error: READ     on CUSTDAT   returned RESP ")
        );
        assert_eq!(
            (customer_error.class().http_status(), customer_error.field()),
            (500, None)
        );
        Ok(())
    }

    // audit: COACTUPC-040
    // audit: COACTUPC-041
    // audit: COACTUPC-043
    // audit: COACTUPC-044
    // audit: COACTUPC-004
    #[test]
    fn an_unchanged_form_is_no_change_and_runs_no_edit() -> TestResult {
        let form = Form::shown()?;
        let current = fetched()?;
        let state = snapshot(UpdateState::ShowDetails)?;
        assert_eq!(
            compare_old_new(&form.input(), &state, &current),
            ChangeCheck::Unchanged
        );
        let mut cased = form.clone();
        cased.texts[STATUS] = "y".to_owned();
        cased.texts[FIRST] = "  IMMANUEL ".to_owned();
        assert_eq!(
            compare_old_new(&cased.input(), &state, &current),
            ChangeCheck::Unchanged
        );
        let mut same_ssn = form.clone();
        same_ssn.ssn = Some(owned(["123", "45", "6789"]));
        assert_eq!(
            compare_old_new(&same_ssn.input(), &state, &current),
            ChangeCheck::Unchanged
        );
        same_ssn.ssn = Some(owned(["123", "45", "6780"]));
        assert_eq!(
            compare_old_new(&same_ssn.input(), &state, &current),
            ChangeCheck::Changed
        );
        for aid in [Aid::Enter, Aid::Pf5] {
            let failure = decided(aid, &form, UpdateState::ShowDetails)?
                .err()
                .ok_or("no change saved")?;
            assert_eq!(
                shown(&failure),
                input_error(NO_CHANGE, UpdateField::ActiveStatus)
            );
            assert_eq!(failure.state(), UpdateState::ShowDetails);
        }
        Ok(())
    }

    // audit: COACTUPC-013
    // audit: COACTUPC-014
    // audit: COACTUPC-082
    #[test]
    fn a_valid_change_is_validated_and_offers_the_save_key() -> TestResult {
        let mut form = Form::changed()?;
        form.texts[CREDIT] = "$3,000.5".to_owned();
        for state in [UpdateState::ShowDetails, UpdateState::ChangesNotOk] {
            let decision = decided(Aid::Enter, &form, state)??;
            let Decision::Respond(outcome) = decision else {
                return Err("saved outside state N".into());
            };
            assert_eq!(outcome.state(), UpdateState::ChangesOkNotConfirmed);
            assert_eq!(outcome.info_message(), "Changes validated.Press F5 to save");
            let amounts = outcome.shown_amounts().ok_or("no amounts")?;
            assert_eq!(amounts.credit_limit, "+      3,000.50");
            assert!(outcome.state().save_key_shown() && outcome.state().cancel_key_shown());
        }
        Ok(())
    }

    // audit: COACTUPC-071
    // audit: COACTUPC-103
    // audit: COACTUPC-104
    // audit: COACTUPC-108
    #[test]
    fn f5_in_state_n_saves_the_received_values() -> TestResult {
        let mut form = Form::changed()?;
        form.texts[STATUS] = "N".to_owned();
        let save = pending(&form)?;
        assert_eq!(save.account_key().get(), 1);
        let effects = save.save_effects(&account()?, &customer()?)?;
        assert_eq!(effects.account().id.get(), 1);
        assert_eq!(effects.account().active_status.as_bytes(), b"N");
        assert_eq!(effects.account().credit_limit, account()?.credit_limit);
        assert_eq!(effects.account().addr_zip.to_text(), "GOLD");
        let CustomerRewrite::Record(rewritten) = effects.customer() else {
            return Err("the customer key changed".into());
        };
        assert_eq!(rewritten.last_name.to_text(), "Kessling");
        assert_eq!(rewritten.addr_zip.as_bytes(), b"90210     ");
        assert_eq!(rewritten.phone_num_1.as_bytes(), b"(201)555-1234  ");
        assert!(rewritten.ssn.expose().get() == 123_456_789, "COACTUPC-104");
        assert_eq!(
            rewritten.dob_yyyy_mm_dd.value(),
            Some(super::date(1961, 6, 8)?)
        );
        let done = save.committed();
        assert_eq!(done.state(), UpdateState::ChangesOkayedAndDone);
        assert_eq!(done.info_message(), "Changes committed to database");
        assert!(!done.state().save_key_shown() && !done.state().cancel_key_shown());
        assert!(
            matches!(done, UpdateOutcome::Committed { .. }),
            "COACTUPC-108"
        );
        Ok(())
    }

    // audit: COACTUPC-101
    // audit: COACTUPC-102
    // audit: COACTUPC-106
    #[test]
    fn failed_saves_set_their_messages_and_states() -> TestResult {
        let save = pending(&Form::changed()?)?;
        let unsuccessful = "Changes unsuccessful. Please try again";
        let lock = save.account_lock_failed();
        assert_eq!(
            shown(&lock),
            (
                "Could not lock account record for update".to_owned(),
                None,
                500
            )
        );
        assert_eq!(lock.state(), UpdateState::LockError);
        assert_eq!(lock.state().info_message(), unsuccessful);
        let customer_lock = save.customer_lock_failed();
        assert_eq!(
            customer_lock.message(),
            "Could not lock customer record for update"
        );
        assert_eq!(customer_lock.state(), UpdateState::ChangesOkayedAndDone);
        let rewrite = save.rewrite_failed();
        assert_eq!(
            shown(&rewrite),
            ("Update of record failed".to_owned(), None, 500)
        );
        assert_eq!(rewrite.state(), UpdateState::ChangesFailed);
        assert_eq!(rewrite.state().info_message(), unsuccessful);
        Ok(())
    }

    // audit: COACTUPC-113
    // audit: COACTUPC-114
    // audit: COACTUPC-115
    // audit: COACTUPC-116
    #[test]
    fn a_record_changed_since_the_fetch_is_a_conflict() -> TestResult {
        let state = snapshot(UpdateState::ChangesOkNotConfirmed)?;
        let (account, customer) = (account()?, customer()?);
        assert_eq!(check_change_in_rec(&state, (&account, &customer)), Ok(()));
        // Only the last four SSN digits are compared (E32).
        let mut first_five = customer.clone();
        first_five.ssn = Ssn::new(PicU::new(987_656_789)?);
        assert_eq!(check_change_in_rec(&state, (&account, &first_five)), Ok(()));
        let mut last_four = customer.clone();
        last_four.ssn = Ssn::new(PicU::new(123_456_780)?);
        let mut balance = account.clone();
        balance.curr_bal = money(19_401)?;
        let mut status = account.clone();
        status.active_status = text("y")?;
        let mut phone = customer.clone();
        phone.phone_num_2 = text("(202)555-9875")?;
        for (changed_account, changed_customer) in [
            (&balance, &customer),
            (&status, &customer),
            (&account, &phone),
            (&account, &last_four),
        ] {
            let failure = check_change_in_rec(&state, (changed_account, changed_customer))
                .err()
                .ok_or("a change by another user passed")?;
            let message = "Record changed by some one else. Please review";
            assert_eq!(shown(&failure), (message.to_owned(), None, 409));
            assert_eq!(failure.class(), FailureClass::Changed);
            assert_eq!(failure.state(), UpdateState::ShowDetails);
        }
        Ok(())
    }

    /// Changes one field of the changed form.
    type Touch = fn(&mut Form);

    fn assert_edit_failures(cases: &[(Touch, &str, UpdateField)]) -> TestResult {
        for (touch, message, field) in cases {
            let mut form = Form::changed()?;
            touch(&mut form);
            assert_eq!(
                edit_failure(&form)?,
                input_error(message, *field),
                "{message}"
            );
        }
        Ok(())
    }

    // audit: COACTUPC-015
    // audit: COACTUPC-016
    // audit: COACTUPC-017
    // audit: COACTUPC-019
    // audit: COACTUPC-021
    // audit: COACTUPC-022
    // audit: COACTUPC-023
    // audit: COACTUPC-049
    // audit: COACTUPC-054
    // audit: COACTUPC-118
    #[test]
    fn account_field_edits_give_their_messages_in_order() -> TestResult {
        assert_edit_failures(&[
            (
                |f| f.texts[STATUS] = String::new(),
                "Account Status must be supplied.",
                UpdateField::ActiveStatus,
            ),
            (
                |f| f.texts[STATUS] = "X".to_owned(),
                "Account Status must be Y or N.",
                UpdateField::ActiveStatus,
            ),
            (
                |f| f.dates[0][0] = String::new(),
                "Open Date : Year must be supplied.",
                UpdateField::OpenYear,
            ),
            (
                |f| f.texts[CREDIT] = String::new(),
                "Credit Limit must be supplied.",
                UpdateField::CreditLimit,
            ),
            (
                |f| f.texts[CREDIT] = "abc".to_owned(),
                "Credit Limit is not valid",
                UpdateField::CreditLimit,
            ),
            (
                |f| f.texts[CASH] = String::new(),
                "Cash Credit Limit must be supplied.",
                UpdateField::CashCreditLimit,
            ),
            (
                |f| f.texts[BALANCE] = "1..2".to_owned(),
                "Current Balance is not valid",
                UpdateField::CurrentBalance,
            ),
            (
                |f| f.texts[CYCLE_CREDIT] = String::new(),
                "Current Cycle Credit Limi must be supplied.",
                UpdateField::CurrentCycleCredit,
            ),
            (
                |f| f.texts[CYCLE_DEBIT] = "x".to_owned(),
                "Current Cycle Debit Limit is not valid",
                UpdateField::CurrentCycleDebit,
            ),
            // The status edit runs first, so its message wins over a later failure.
            (
                |f| {
                    f.texts[STATUS] = String::new();
                    f.texts[CREDIT] = String::new();
                },
                "Account Status must be supplied.",
                UpdateField::ActiveStatus,
            ),
        ])
    }

    // audit: COACTUPC-026
    // audit: COACTUPC-027
    // audit: COACTUPC-028
    // audit: COACTUPC-029
    // audit: COACTUPC-030
    // audit: COACTUPC-031
    // audit: COACTUPC-032
    // audit: COACTUPC-033
    // audit: COACTUPC-034
    // audit: COACTUPC-037
    // audit: COACTUPC-038
    // audit: COACTUPC-065
    // audit: COACTUPC-066
    // audit: COACTUPC-067
    #[test]
    fn customer_field_edits_give_their_messages() -> TestResult {
        assert_edit_failures(&[
            (
                |f| f.texts[FICO] = String::new(),
                "FICO Score must be supplied.",
                UpdateField::FicoScore,
            ),
            (
                |f| f.texts[FICO] = "100".to_owned(),
                "FICO Score: should be between 300 and 850",
                UpdateField::FicoScore,
            ),
            (
                |f| f.texts[FIRST] = "Imm4nuel".to_owned(),
                "First Name can have alphabets only.",
                UpdateField::FirstName,
            ),
            (
                |f| f.texts[MIDDLE] = "M.".to_owned(),
                "Middle Name can have alphabets only.",
                UpdateField::MiddleName,
            ),
            (
                |f| f.texts[LAST] = String::new(),
                "Last Name must be supplied.",
                UpdateField::LastName,
            ),
            (
                |f| f.texts[LINE_1] = String::new(),
                "Address Line 1 must be supplied.",
                UpdateField::AddressLine1,
            ),
            (
                |f| f.texts[STATE] = "C1".to_owned(),
                "State can have alphabets only.",
                UpdateField::State,
            ),
            (
                |f| f.texts[STATE] = "ZZ".to_owned(),
                "State: is not a valid state code",
                UpdateField::State,
            ),
            (
                |f| f.texts[ZIP] = "9021A".to_owned(),
                "Zip must be all numeric.",
                UpdateField::Zip,
            ),
            (
                |f| f.texts[ZIP] = "00000".to_owned(),
                "Zip must not be zero.",
                UpdateField::Zip,
            ),
            (
                |f| f.texts[CITY] = "Altenwerth 2".to_owned(),
                "City can have alphabets only.",
                UpdateField::City,
            ),
            (
                |f| f.texts[COUNTRY] = String::new(),
                "Country must be supplied.",
                UpdateField::Country,
            ),
            (
                |f| f.texts[EFT] = "005358175X".to_owned(),
                "EFT Account Id must be all numeric.",
                UpdateField::EftAccountId,
            ),
            (
                |f| f.texts[PRIMARY] = "0".to_owned(),
                "Primary Card Holder must be supplied.",
                UpdateField::PrimaryCardHolder,
            ),
            (
                |f| f.texts[ZIP] = "10001".to_owned(),
                "Invalid zip code for state",
                UpdateField::State,
            ),
        ])
    }

    // audit: COACTUPC-024
    // audit: COACTUPC-025
    // audit: COACTUPC-035
    // audit: COACTUPC-036
    // audit: COACTUPC-056
    // audit: COACTUPC-057
    // audit: COACTUPC-060
    // audit: COACTUPC-061
    // audit: COACTUPC-130
    #[test]
    fn phone_ssn_and_birth_date_edits_give_their_messages() -> TestResult {
        assert_edit_failures(&[
            (
                |f| f.phones[0][0] = String::new(),
                "Phone Number 1: Area code must be supplied.",
                UpdateField::Phone1Area,
            ),
            (
                |f| f.phones[1][0] = "200".to_owned(),
                "Phone Number 2: Not valid North America general purpose area code",
                UpdateField::Phone2Area,
            ),
            (
                |f| f.ssn = Some(owned(["", "45", "6789"])),
                "SSN: First 3 chars must be supplied.",
                UpdateField::SsnPart1,
            ),
            (
                |f| f.ssn = Some(owned(["666", "45", "6789"])),
                "SSN: First 3 chars: should not be 000, 666, or between 900 and 999",
                UpdateField::SsnPart1,
            ),
            (
                |f| f.dates[3][2] = String::new(),
                "Date of Birth : Day must be supplied.",
                UpdateField::DobDay,
            ),
            // The clock reads 2022-07-18: a birth date on that day is not in the past.
            (
                |f| f.dates[3] = owned(["2022", "07", "18"]),
                "Date of Birth:cannot be in the future",
                UpdateField::DobYear,
            ),
        ])
    }

    // audit: COACTUPC-079
    // audit: COACTUPC-082
    #[test]
    fn each_state_has_its_code_info_message_and_keys() {
        let cases = [
            (
                UpdateState::NotFetched,
                ' ',
                "Enter or update id of account to update",
                false,
                false,
            ),
            (
                UpdateState::ShowDetails,
                'S',
                "Update account details presented above.",
                false,
                false,
            ),
            (
                UpdateState::ChangesNotOk,
                'E',
                "Update account details presented above.",
                false,
                true,
            ),
            (
                UpdateState::ChangesOkNotConfirmed,
                'N',
                "Changes validated.Press F5 to save",
                true,
                true,
            ),
            (
                UpdateState::ChangesOkayedAndDone,
                'C',
                "Changes committed to database",
                false,
                false,
            ),
            (
                UpdateState::LockError,
                'L',
                "Changes unsuccessful. Please try again",
                false,
                true,
            ),
            (
                UpdateState::ChangesFailed,
                'F',
                "Changes unsuccessful. Please try again",
                false,
                true,
            ),
        ];
        for (state, code, info, save, cancel) in cases {
            assert_eq!(state.code(), code);
            assert_eq!(state.info_message(), info);
            assert_eq!(
                (state.save_key_shown(), state.cancel_key_shown()),
                (save, cancel)
            );
            assert_eq!(UpdateState::from_code(&code.to_string()), Some(state));
        }
        assert_eq!(UpdateState::from_code("Q"), None);
    }
}

mod cocrdlic {
    use domain::copybook::PicU;
    use domain::online::cocrdlic::{
        CardFilters, CardListRow, CardPage, CardRef, DEFAULT_PAGE_SIZE, Direction, LastPage,
        ListFailure, ListMessage, ListPlan, ListState, NextPage, PageNumber, Read, edit_filters,
        page, plan,
    };
    use domain::records::CardRecord;

    use super::{TestResult, fixture};

    const INFO: Option<&str> = Some("TYPE S FOR DETAIL, U TO UPDATE ANY RECORD");

    /// The 50 fixture cards in key order, each with the handle of its line number.
    fn cards() -> Result<Vec<(CardRef, CardRecord)>, Box<dyn std::error::Error>> {
        Ok(fixture::<CardRecord>("carddata.txt")?
            .into_iter()
            .zip(1_i64..)
            .map(|(record, handle)| (CardRef::new(handle), record))
            .collect())
    }

    /// Answers `plan` as the card repository would: the filtered rows of its read and the
    /// unfiltered card after the last row read.
    fn run(plan: &ListPlan, filters: &CardFilters, cards: &[(CardRef, CardRecord)]) -> CardPage {
        let index = |start: Option<CardRef>| {
            start.and_then(|key| cards.iter().position(|(card_ref, _)| *card_ref == key))
        };
        let row = |(card_ref, record): &(CardRef, CardRecord)| CardListRow::new(*card_ref, record);
        let rows: Vec<CardListRow> = match plan.read() {
            Read::Forward { start } => cards
                .iter()
                .skip(index(start).unwrap_or(0))
                .filter(|(_, record)| filters.includes(record))
                .take(plan.limit())
                .map(row)
                .collect(),
            Read::Backward { start } => cards
                .iter()
                .take(index(start).map_or(0, |at| at + 1))
                .rev()
                .filter(|(_, record)| filters.includes(record))
                .take(plan.limit())
                .map(row)
                .collect(),
        };
        let lookahead = plan.lookahead_after(&rows).and_then(|after| {
            cards
                .iter()
                .skip_while(|(card_ref, _)| *card_ref != after.card_ref)
                .nth(1)
                .map(|(card_ref, _)| *card_ref)
        });
        page(plan, rows, lookahead)
    }

    fn handles(page: &CardPage) -> Vec<i64> {
        page.rows().iter().map(|row| row.card_ref.get()).collect()
    }

    // audit: COCRDLIC-005
    // audit: COCRDLIC-007
    // audit: COCRDLIC-010
    #[test]
    fn blank_and_zero_filters_list_every_card_and_valid_ones_match_both() -> TestResult {
        for (acct, card) in [
            (None, None),
            (Some(""), Some("")),
            (Some("00000000000"), Some("0000000000000000")),
        ] {
            assert_eq!(edit_filters(acct, card)?, CardFilters::default());
        }
        let cards = cards()?;
        let (_, first) = cards.first().ok_or("no fixture card")?;
        let (_, second) = cards.get(1).ok_or("no second fixture card")?;
        let by_account = edit_filters(Some("00000000050"), None)?;
        assert_eq!(by_account.account().map(PicU::get), Some(50));
        assert!(by_account.includes(first) && !by_account.includes(second));
        let by_card = edit_filters(None, Some("0683586198171516"))?;
        assert!(by_card.card() == Some(&second.num), "COCRDLIC-010");
        assert!(by_card.includes(second) && !by_card.includes(first));
        let crossed = edit_filters(Some("00000000050"), Some("0683586198171516"))?;
        assert!(!crossed.includes(first) && !crossed.includes(second));
        let listed = run(
            &plan(None, ListState::fresh(), DEFAULT_PAGE_SIZE),
            &by_account,
            &cards,
        );
        assert_eq!(handles(&listed), vec![1]);
        assert_eq!(listed.message(), Some("NO MORE RECORDS TO SHOW"));
        Ok(())
    }

    // audit: COCRDLIC-017
    // audit: COCRDLIC-019
    // audit: COCRDLIC-022
    // audit: COCRDLIC-023
    // audit: COCRDLIC-032
    // audit: COCRDLIC-033
    #[test]
    fn the_first_pages_list_seven_redacted_cards_each() -> TestResult {
        assert_eq!(DEFAULT_PAGE_SIZE, 7);
        let cards = cards()?;
        let none = CardFilters::default();
        let fresh = plan(None, ListState::fresh(), DEFAULT_PAGE_SIZE);
        assert_eq!(fresh.read(), Read::Forward { start: None });
        assert_eq!((fresh.page_size(), fresh.limit()), (7, 7));
        let first = run(&fresh, &none, &cards);
        assert_eq!(handles(&first), (1..=7).collect::<Vec<i64>>());
        assert_eq!(first.page_no(), PageNumber::FIRST);
        assert_eq!(first.first_key(), Some(CardRef::new(1)));
        assert_eq!(first.last_key(), Some(CardRef::new(8)));
        assert_eq!(first.next_page(), NextPage::Exists);
        assert_eq!(first.message(), None);
        assert_eq!(first.info_message(), INFO);
        let row = first.rows().first().ok_or("no first row")?;
        assert_eq!(row.card_num.to_string(), "************5740");
        assert!(
            !format!("{first:?}").contains("0500024453765740"),
            "COCRDLIC-032"
        );
        assert_eq!(row.acct_id.get(), 50);
        assert_eq!(row.status.as_bytes(), b"Y");
        let second = run(
            &plan(Some(Direction::Forward), first.state(), 7),
            &none,
            &cards,
        );
        assert_eq!(handles(&second), (8..=14).collect::<Vec<i64>>());
        assert_eq!(second.page_no().get(), 2);
        assert_eq!(second.last_key(), Some(CardRef::new(15)));
        Ok(())
    }

    // audit: COCRDLIC-020
    // audit: COCRDLIC-024
    // audit: COCRDLIC-025
    #[test]
    fn the_last_page_says_no_more_records_then_no_more_pages() -> TestResult {
        let cards = cards()?;
        let none = CardFilters::default();
        let mut shown = run(&plan(None, ListState::fresh(), 7), &none, &cards);
        for _ in 2..=8 {
            shown = run(
                &plan(Some(Direction::Forward), shown.state(), 7),
                &none,
                &cards,
            );
        }
        assert_eq!(handles(&shown), vec![50]);
        assert_eq!(shown.page_no().get(), 8);
        assert_eq!(shown.next_page(), NextPage::NotExists);
        assert_eq!(shown.message_kind(), Some(ListMessage::NoMoreRecords));
        assert_eq!(shown.last_page(), LastPage::Shown);
        let again = run(
            &plan(Some(Direction::Forward), shown.state(), 7),
            &none,
            &cards,
        );
        assert_eq!(again.message(), Some("NO MORE PAGES TO DISPLAY"));
        assert_eq!(again.info_message(), None);
        Ok(())
    }

    // audit: COCRDLIC-021
    // audit: COCRDLIC-026
    // audit: COCRDLIC-027
    // audit: COCRDLIC-028
    // audit: COCRDLIC-034
    #[test]
    fn empty_and_backward_pages_carry_their_messages() -> TestResult {
        let cards = cards()?;
        let none = CardFilters::default();
        let crossed = edit_filters(Some("00000000050"), Some("0683586198171516"))?;
        let empty = run(&plan(None, ListState::fresh(), 7), &crossed, &cards);
        assert!(empty.rows().is_empty(), "COCRDLIC-021");
        assert_eq!(
            empty.message(),
            Some("NO RECORDS FOUND FOR THIS SEARCH CONDITION.")
        );
        assert_eq!(empty.info_message(), None);
        let first = run(&plan(None, ListState::fresh(), 7), &none, &cards);
        let back = plan(Some(Direction::Backward), first.state(), 7);
        assert_eq!(
            back.read(),
            Read::Forward {
                start: Some(CardRef::new(1))
            }
        );
        let again = run(&back, &none, &cards);
        assert_eq!(again.message(), Some("NO PREVIOUS PAGES TO DISPLAY"));
        let second = run(
            &plan(Some(Direction::Forward), first.state(), 7),
            &none,
            &cards,
        );
        let previous = run(
            &plan(Some(Direction::Backward), second.state(), 7),
            &none,
            &cards,
        );
        assert_eq!(handles(&previous), (1..=7).collect::<Vec<i64>>());
        assert_eq!(previous.page_no(), PageNumber::FIRST);
        assert_eq!(previous.message(), Some("NO PREVIOUS PAGES TO DISPLAY"));
        assert_eq!(
            ListFailure::file_error().message(),
            ListMessage::FileError.text()
        );
        Ok(())
    }

    // The filter-error rows are HALTED(X-COCRDLIC-H2): the evident-path answers run here for
    // coverage only, with no value asserted.
    #[test]
    fn non_digit_filters_run_without_parity_assertions() {
        for (acct, card) in [
            (Some("50"), None),
            (None, Some("1234")),
            (Some("x"), Some("y")),
        ] {
            let _ = edit_filters(acct, card);
        }
    }
}

mod cocrdslc {
    use domain::copybook::PicU;
    use domain::online::cocrdlic::CardRef;
    use domain::online::cocrdslc::{
        CardKey, CardLookup, CardNumberDisclosure, DetailField, FailureClass, disclose, edit, view,
    };
    use domain::online::cosgn00c::UserType;
    use domain::records::CardRecord;

    use super::{TestResult, date, fixture};

    const CARD: &str = "0500024453765740";

    fn first_card() -> Result<CardRecord, Box<dyn std::error::Error>> {
        Ok(fixture::<CardRecord>("carddata.txt")?
            .into_iter()
            .next()
            .ok_or("no fixture card")?)
    }

    // audit: COCRDSLC-006
    // audit: COCRDSLC-007
    // audit: COCRDSLC-008
    // audit: COCRDSLC-009
    // audit: COCRDSLC-010
    // audit: COCRDSLC-013
    // audit: COCRDSLC-028
    #[test]
    fn the_edits_take_exactly_one_card_key() -> TestResult {
        let (acct, key) = edit("00000000050", Some(CARD), None)?;
        assert_eq!(acct.get(), 50);
        assert!(
            matches!(&key, CardKey::ByNumber(number) if number.expose().as_bytes() == CARD.as_bytes())
        );
        let (_, by_ref) = edit("00000000050", None, Some(CardRef::new(1)))?;
        assert_eq!(by_ref, CardKey::ByRef(CardRef::new(1)));
        let cases = [
            (
                "00000000050",
                Some(CARD),
                Some(CardRef::new(1)),
                "CARD ID FILTER,IF SUPPLIED MUST BE A 16 DIGIT NUMBER",
                DetailField::CardNum,
            ),
            ("", None, None, "No input received", DetailField::AcctId),
            (
                "",
                Some(CARD),
                None,
                "Account number not provided",
                DetailField::AcctId,
            ),
            (
                "00000000000",
                None,
                Some(CardRef::new(1)),
                "Account number not provided",
                DetailField::AcctId,
            ),
            (
                "00000000050",
                None,
                None,
                "Card number not provided",
                DetailField::CardNum,
            ),
        ];
        for (acct, card_num, card_ref, message, field) in cases {
            let failure = edit(acct, card_num, card_ref)
                .err()
                .ok_or("the edit passed")?;
            assert_eq!(failure.message(), message);
            assert_eq!(failure.field(), field);
            assert_eq!(failure.class(), FailureClass::Input);
            assert_eq!(failure.class().http_status(), 400);
        }
        assert_eq!(DetailField::CardNum.name(), "card_num");
        Ok(())
    }

    // audit: COCRDSLC-014
    // audit: COCRDSLC-018
    // audit: COCRDSLC-019
    // audit: COCRDSLC-020
    // audit: COCRDSLC-022
    // audit: COCRDSLC-023
    #[test]
    fn a_found_card_is_disclosed_only_to_an_admin() -> TestResult {
        let record = first_card()?;
        let acct_id = PicU::new(7)?;
        let found = || CardLookup::Found {
            acct_id,
            card_ref: CardRef::new(1),
            record: &record,
        };
        let shown = view(&UserType::USER, found())?;
        assert_eq!(shown.acct_id.get(), 7);
        assert_eq!(shown.card_ref, CardRef::new(1));
        assert_eq!(shown.embossed_name.to_text(), "Aniya Von");
        assert_eq!(shown.expiration_date, Some(date(2023, 3, 9)?));
        assert_eq!(shown.status.as_bytes(), b"Y");
        assert_eq!(shown.info_message, "   Displaying requested details");
        let CardNumberDisclosure::Redacted(number) = &shown.card_num else {
            return Err("COCRDSLC-023: a user saw the full number".into());
        };
        assert_eq!(number.to_string(), "************5740");
        assert!(!format!("{shown:?}").contains(CARD), "COCRDSLC-023");
        let admin = view(&UserType::ADMIN, found())?;
        assert!(
            matches!(admin.card_num, CardNumberDisclosure::Full(_)),
            "COCRDSLC-022"
        );
        for user_type in [
            UserType::USER,
            UserType::from_code(domain::copybook::PicX::from_array([b' '])),
        ] {
            assert!(
                matches!(
                    disclose(&user_type, &record.num),
                    CardNumberDisclosure::Redacted(_)
                ),
                "COCRDSLC-023"
            );
        }
        assert!(
            matches!(
                disclose(&UserType::ADMIN, &record.num),
                CardNumberDisclosure::Full(_)
            ),
            "COCRDSLC-022"
        );
        Ok(())
    }

    // audit: COCRDSLC-015
    // audit: COCRDSLC-016
    #[test]
    fn a_missing_or_failed_card_read_reports_on_the_account() -> TestResult {
        let missing = view(&UserType::USER, CardLookup::NotFound)
            .err()
            .ok_or("a missing card was shown")?;
        assert_eq!(
            missing.message(),
            "Did not find cards for this search condition"
        );
        assert_eq!(
            (missing.field(), missing.class().http_status()),
            (DetailField::AcctId, 404)
        );
        let failed = view(&UserType::ADMIN, CardLookup::Error)
            .err()
            .ok_or("a failed read was shown")?;
        // HALTED(H4): bytes 49-58 and 66-75 hold CICS codes and are not asserted.
        assert_eq!(
            failed.message().get(..48),
            Some("File Error: READ     on CARDDAT   returned RESP ")
        );
        assert_eq!(
            (failed.field(), failed.class().http_status()),
            (DetailField::AcctId, 500)
        );
        Ok(())
    }
}

mod cocrdupc {
    use domain::copybook::{PicU, PicX};
    use domain::online::cocrdlic::CardRef;
    use domain::online::cocrdupc::{
        Aid, CardKey, CardLookup, CardSaveEffects, CardSnapshot, CardUpdateFailure,
        CardUpdateField, CardUpdateInput, CardUpdateState, CardUpdateView, Decision, FailureClass,
        decide, edit, edit_keys, fetch,
    };
    use domain::records::CardRecord;

    use super::{TestResult, date, fixture};

    type Outcome<T> = Result<T, Box<dyn std::error::Error>>;

    const HANDLE: CardRef = CardRef::new(1);

    /// `carddata.txt:1` as fetched: the name upper-cased.
    const AS_FETCHED: CardUpdateInput<'static> = CardUpdateInput {
        embossed_name: "aniya von",
        status: "Y",
        expiry_month: "03",
        expiry_year: "2023",
    };

    const VALID_CHANGE: CardUpdateInput<'static> = CardUpdateInput {
        embossed_name: "Aniya Vonn",
        status: "N",
        expiry_month: "06",
        expiry_year: "2030",
    };

    fn first_card() -> Outcome<CardRecord> {
        Ok(fixture::<CardRecord>("carddata.txt")?
            .into_iter()
            .next()
            .ok_or("no fixture card")?)
    }

    fn shown(record: &CardRecord) -> Outcome<CardUpdateView> {
        let found = CardLookup::Found {
            acct_id: PicU::new(50)?,
            card_ref: HANDLE,
            record,
        };
        Ok(fetch(&CardKey::ByRef(HANDLE), found)?)
    }

    fn snapshot_in(state: CardUpdateState) -> Outcome<CardSnapshot> {
        Ok(shown(&first_card()?)?.snapshot.with_state(state))
    }

    fn save_of(input: &CardUpdateInput<'_>) -> Outcome<CardSaveEffects> {
        match decide(
            Aid::Pf5,
            &snapshot_in(CardUpdateState::ChangesOkNotConfirmed)?,
            input,
        )? {
            Decision::Save(effects) => Ok(effects),
            Decision::Respond(outcome) => Err(format!("no save: {outcome:?}").into()),
        }
    }

    fn input_failure(
        failure: Option<CardUpdateFailure>,
    ) -> Option<(&'static str, Option<CardUpdateField>, u16)> {
        failure.map(|f| (f.message(), f.field(), f.class().http_status()))
    }

    // audit: COCRDUPC-010
    // audit: COCRDUPC-011
    // audit: COCRDUPC-012
    // audit: COCRDUPC-013
    // audit: COCRDUPC-014
    // audit: COCRDUPC-015
    #[test]
    fn the_search_keys_are_edited_account_first() -> TestResult {
        let card = "0500024453765740";
        let cases = [
            ("", Some(""), "No input received", CardUpdateField::AcctId),
            (
                "00000000000",
                Some(card),
                "Account number not provided",
                CardUpdateField::AcctId,
            ),
            (
                "1234567890A",
                Some(card),
                "ACCOUNT FILTER,IF SUPPLIED MUST BE A 11 DIGIT NUMBER",
                CardUpdateField::AcctId,
            ),
            (
                "00000000050",
                Some(""),
                "Card number not provided",
                CardUpdateField::CardNum,
            ),
            (
                "00000000050",
                Some("0000000000000000"),
                "Card number not provided",
                CardUpdateField::CardNum,
            ),
            (
                "00000000050",
                Some("050002445376574X"),
                "CARD ID FILTER,IF SUPPLIED MUST BE A 16 DIGIT NUMBER",
                CardUpdateField::CardNum,
            ),
        ];
        for (acct, card_num, message, field) in cases {
            let failure = edit_keys(acct, card_num, None)
                .err()
                .ok_or("the keys passed")?;
            assert_eq!(
                input_failure(Some(failure)),
                Some((message, Some(field), 400))
            );
            assert_eq!(failure.state(), CardUpdateState::NotFetched);
            assert_eq!(
                failure.state().info_message(),
                "Please enter Account and Card Number"
            );
        }
        let (acct, key) = edit_keys("00000000050", None, Some(HANDLE))?;
        assert_eq!((acct.get(), key), (50, CardKey::ByRef(HANDLE)));
        Ok(())
    }

    // audit: COCRDUPC-002
    // audit: COCRDUPC-016
    // audit: COCRDUPC-017
    // audit: COCRDUPC-018
    // audit: COCRDUPC-019
    // audit: COCRDUPC-042
    // audit: COCRDUPC-044
    #[test]
    fn the_fetch_shows_the_upper_cased_card_redacted() -> TestResult {
        let view = shown(&first_card()?)?;
        assert_eq!(view.card_ref, HANDLE);
        assert_eq!(view.acct_id.get(), 50);
        assert_eq!(view.card_num.to_string(), "************5740");
        assert!(
            !format!("{view:?}").contains("0500024453765740"),
            "COCRDUPC-042"
        );
        assert_eq!(view.embossed_name.to_text(), "ANIYA VON");
        assert_eq!(view.expiration_date, Some(date(2023, 3, 9)?));
        assert_eq!(view.status.as_bytes(), b"Y");
        assert_eq!(view.info_message, "Details of selected card shown above");
        assert_eq!(view.snapshot.state(), CardUpdateState::ShowDetails);
        assert_eq!(view.snapshot.embossed_name().to_text(), "ANIYA VON");
        assert_eq!(view.snapshot.expiration_date(), Some(date(2023, 3, 9)?));
        let key = CardKey::ByRef(HANDLE);
        let missing = fetch(&key, CardLookup::NotFound).err();
        assert_eq!(
            input_failure(missing).map(|(message, _, status)| (message, status)),
            Some(("Did not find cards for this search condition", 404))
        );
        let failed = fetch(&key, CardLookup::Error)
            .err()
            .ok_or("a failed read was shown")?;
        // HALTED(H4): the CICS codes at bytes 49-58 and 66-75 are not asserted.
        assert_eq!(
            failed.message().get(..48),
            Some("File Error: READ     on CARDDAT   returned RESP ")
        );
        assert_eq!(
            (failed.field(), failed.class()),
            (Some(CardUpdateField::AcctId), FailureClass::Store)
        );
        Ok(())
    }

    // audit: COCRDUPC-020
    // audit: COCRDUPC-021
    // audit: COCRDUPC-022
    // audit: COCRDUPC-007
    #[test]
    fn an_unchanged_or_case_only_change_is_no_change() -> TestResult {
        let case_only = CardUpdateInput {
            status: "y",
            ..AS_FETCHED
        };
        for (aid, state, input) in [
            (Aid::Enter, CardUpdateState::ShowDetails, AS_FETCHED),
            (Aid::Pf5, CardUpdateState::ShowDetails, AS_FETCHED),
            (Aid::Enter, CardUpdateState::ChangesNotOk, case_only),
        ] {
            let failure = decide(aid, &snapshot_in(state)?, &input)
                .err()
                .ok_or("no change was accepted")?;
            let message = "No change detected with respect to values fetched.";
            assert_eq!(
                input_failure(Some(failure)),
                Some((message, Some(CardUpdateField::EmbossedName), 400))
            );
            assert_eq!(failure.state(), state);
        }
        Ok(())
    }

    // audit: COCRDUPC-023
    // audit: COCRDUPC-024
    // audit: COCRDUPC-025
    // audit: COCRDUPC-027
    // audit: COCRDUPC-028
    // audit: COCRDUPC-029
    // audit: COCRDUPC-030
    // audit: COCRDUPC-031
    // audit: COCRDUPC-032
    #[test]
    fn each_edit_gives_its_message_name_first() -> TestResult {
        const NAME: CardUpdateField = CardUpdateField::EmbossedName;
        const STATUS: CardUpdateField = CardUpdateField::Status;
        const MONTH: CardUpdateField = CardUpdateField::ExpiryMonth;
        const YEAR: CardUpdateField = CardUpdateField::ExpiryYear;
        const NO_NAME: &str = "Card name not provided";
        const BAD_NAME: &str = "Card name can only contain alphabets and spaces";
        const BAD_STATUS: &str = "Card Active Status must be Y or N";
        const BAD_MONTH: &str = "Card expiry month must be between 1 and 12";
        const BAD_YEAR: &str = "Invalid card expiry year";
        let zeros = "0".repeat(50);
        let cases = [
            (["", "N", "06", "2030"], NO_NAME, NAME),
            ([zeros.as_str(), "N", "06", "2030"], NO_NAME, NAME),
            (["O'Hara", "N", "06", "2030"], BAD_NAME, NAME),
            (["Aniya Vonn", "", "06", "2030"], BAD_STATUS, STATUS),
            (["Aniya Vonn", "0", "06", "2030"], BAD_STATUS, STATUS),
            (["Aniya Vonn", "y", "06", "2030"], BAD_STATUS, STATUS),
            (["Aniya Vonn", "N", "", "2030"], BAD_MONTH, MONTH),
            (["Aniya Vonn", "N", "00", "2030"], BAD_MONTH, MONTH),
            (["Aniya Vonn", "N", "13", "2030"], BAD_MONTH, MONTH),
            (["Aniya Vonn", "N", "06", ""], BAD_YEAR, YEAR),
            (["Aniya Vonn", "N", "06", "0000"], BAD_YEAR, YEAR),
            (["Aniya Vonn", "N", "06", "1949"], BAD_YEAR, YEAR),
            (["Aniya Vonn", "N", "06", "2100"], BAD_YEAR, YEAR),
            (["", "X", "13", "1949"], NO_NAME, NAME),
        ];
        let shown = snapshot_in(CardUpdateState::ShowDetails)?;
        for ([embossed_name, status, expiry_month, expiry_year], message, field) in cases {
            let input = CardUpdateInput {
                embossed_name,
                status,
                expiry_month,
                expiry_year,
            };
            assert_eq!(
                input_failure(edit(&input).err()),
                Some((message, Some(field), 400))
            );
            let decided = decide(Aid::Enter, &shown, &input).err();
            let decided = decided.map(|f| (f.message(), f.state()));
            assert_eq!(decided, Some((message, CardUpdateState::ChangesNotOk)));
        }
        let changes = edit(&VALID_CHANGE)?;
        assert_eq!(changes.embossed_name().to_text(), "Aniya Vonn");
        assert_eq!((changes.expiry_month(), changes.expiry_year()), (6, 2030));
        for (expiry_month, expiry_year) in [("01", "1950"), ("12", "2099")] {
            let edge = CardUpdateInput {
                expiry_month,
                expiry_year,
                ..VALID_CHANGE
            };
            assert!(edit(&edge).is_ok(), "COCRDUPC-030, COCRDUPC-032");
        }
        Ok(())
    }

    // audit: COCRDUPC-034
    // audit: COCRDUPC-035
    #[test]
    fn a_valid_change_is_validated_then_held_in_state_n() -> TestResult {
        let decision = decide(
            Aid::Enter,
            &snapshot_in(CardUpdateState::ShowDetails)?,
            &VALID_CHANGE,
        )?;
        let Decision::Respond(outcome) = decision else {
            return Err("ENTER saved".into());
        };
        assert_eq!(outcome.state(), CardUpdateState::ChangesOkNotConfirmed);
        assert_eq!(outcome.info_message(), "Changes validated.Press F5 to save");
        assert!(outcome.confirmation_keys_shown(), "COCRDUPC-034");
        let held = decide(
            Aid::Enter,
            &snapshot_in(CardUpdateState::ChangesOkNotConfirmed)?,
            &VALID_CHANGE,
        )?;
        let Decision::Respond(again) = held else {
            return Err("ENTER in state N saved".into());
        };
        assert_eq!(again.state(), CardUpdateState::ChangesOkNotConfirmed);
        Ok(())
    }

    // audit: COCRDUPC-036
    // audit: COCRDUPC-037
    // audit: COCRDUPC-038
    // audit: COCRDUPC-039
    // audit: COCRDUPC-040
    #[test]
    fn f5_in_state_n_rewrites_the_held_card() -> TestResult {
        let held = first_card()?;
        let save = save_of(&VALID_CHANGE)?;
        assert_eq!(save.check_change_in_rec(&held), Ok(()));
        let rewritten = save.rewritten(&held)?;
        assert!(rewritten.num == held.num, "COCRDUPC-038");
        assert_eq!(rewritten.acct_id.get(), 50);
        assert_eq!(rewritten.embossed_name.to_text(), "Aniya Vonn");
        assert_eq!(rewritten.active_status.as_bytes(), b"N");
        assert_eq!(rewritten.expiraion_date.value(), Some(date(2030, 6, 9)?));
        let done = save.committed();
        assert_eq!(done.state(), CardUpdateState::ChangesOkayedAndDone);
        assert_eq!(done.info_message(), "Changes committed to database");
        let mut changed = held.clone();
        changed.embossed_name = PicX::from_text("Someone Else")?;
        let conflict = save.check_change_in_rec(&changed).err();
        let message = "Record changed by some one else. Please review";
        assert_eq!(
            input_failure(conflict).map(|(text, _, status)| (text, status)),
            Some((message, 409))
        );
        let lock = CardUpdateFailure::lock_failed();
        assert_eq!(
            (lock.message(), lock.class().http_status()),
            ("Could not lock record for update", 500)
        );
        assert_eq!(
            lock.state().info_message(),
            "Changes unsuccessful. Please try again"
        );
        let rewrite = CardUpdateFailure::rewrite_failed();
        assert_eq!(
            (rewrite.message(), rewrite.state()),
            ("Update of record failed", CardUpdateState::UpdateFailed)
        );
        Ok(())
    }
}

mod cotrn00c {
    use domain::copybook::{PicU, PicX};
    use domain::online::cotrn00c::{
        ALREADY_AT_BOTTOM, ALREADY_AT_TOP, Browse, BrowseKey, DEFAULT_PAGE_SIZE, Direction,
        FailureClass, ListKey, NextPage, PageRows, Plan, REACHED_BOTTOM, TranListField,
        TranListState, TranPage, edit_start_tran_id, lookup_failure, page, plan, row_from,
    };
    use domain::records::TransactionRecord;

    use super::{TestResult, date, fixture};

    type Outcome<T> = Result<T, Box<dyn std::error::Error>>;

    /// The 300 `dailytran.txt` records, whose layout is the `CVTRA05Y` one, in key order.
    fn transactions() -> Outcome<Vec<TransactionRecord>> {
        fixture::<TransactionRecord>("dailytran.txt")
    }

    /// Answers `browse` as the transaction repository would: from its key, in its direction,
    /// at most its limit.
    fn read(
        browse: &Browse,
        all: &[TransactionRecord],
    ) -> Vec<domain::online::cotrn00c::TranListRow> {
        let key = browse.start().query_key();
        let mut rows: Vec<&TransactionRecord> = match browse.direction() {
            Direction::Forward => all.iter().filter(|t| t.id >= key).collect(),
            Direction::Backward => all.iter().rev().filter(|t| t.id <= key).collect(),
        };
        rows.truncate(browse.limit());
        rows.into_iter().map(row_from).collect()
    }

    fn browse_of(planned: Plan) -> Outcome<Browse> {
        match planned {
            Plan::Read(browse) => Ok(browse),
            Plan::Answer(answer) => Err(format!("expected a read, got {answer:?}").into()),
        }
    }

    fn answer_of(planned: Plan) -> Outcome<TranPage> {
        match planned {
            Plan::Answer(answer) => Ok(answer),
            Plan::Read(browse) => Err(format!("expected an answer, got {browse:?}").into()),
        }
    }

    fn shown_ids(answer: &TranPage) -> Outcome<Vec<String>> {
        let PageRows::Read(slots) = answer.rows() else {
            return Err("the rows were kept".into());
        };
        Ok(slots
            .iter()
            .flatten()
            .map(|row| row.tran_id().to_text())
            .collect())
    }

    // audit: COTRN00C-011
    // audit: COTRN00C-012
    // audit: COTRN00C-013
    #[test]
    fn the_search_field_is_blank_or_sixteen_digits() -> TestResult {
        for blank in ["", "                ", "\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0"] {
            assert_eq!(edit_start_tran_id(blank)?, None);
        }
        let key = edit_start_tran_id("0000000000683580")?;
        assert_eq!(key, Some(PicX::from_text("0000000000683580")?));
        for bad in ["683580", "000000000068358A", " 000000000068358"] {
            let failure = edit_start_tran_id(bad).err().ok_or("a bad key passed")?;
            assert_eq!(failure.message(), "Tran ID must be Numeric ...");
            assert_eq!(failure.field(), Some(TranListField::StartTranId));
            assert_eq!(failure.class(), FailureClass::Input);
            assert_eq!(failure.class().http_status(), 400);
        }
        assert_eq!(
            lookup_failure().message(),
            "Unable to lookup transaction..."
        );
        assert_eq!(lookup_failure().class().http_status(), 500);
        Ok(())
    }

    // audit: COTRN00C-014
    // audit: COTRN00C-016
    // audit: COTRN00C-028
    // audit: COTRN00C-029
    // audit: COTRN00C-031
    // audit: COTRN00C-032
    // audit: COTRN00C-034
    #[test]
    fn enter_lists_ten_fixture_transactions_with_their_display_fields() -> TestResult {
        assert_eq!(DEFAULT_PAGE_SIZE, 10);
        let all = transactions()?;
        let browse = browse_of(plan(
            ListKey::Enter,
            None,
            TranListState::default(),
            DEFAULT_PAGE_SIZE,
        )?)?;
        assert_eq!(browse.start(), &BrowseKey::Lowest);
        assert_eq!((browse.page_size(), browse.limit()), (10, 11));
        let first = page(browse.clone(), read(&browse, &all))?;
        let expected: Vec<String> = all.iter().take(10).map(|t| t.id.to_text()).collect();
        assert_eq!(shown_ids(&first)?, expected);
        assert_eq!(first.message(), None);
        assert_eq!(first.state().page_no(), PicU::new(1)?);
        assert_eq!(first.state().next_page(), NextPage::Yes);
        assert_eq!(
            first.state().first_key().map(PicX::to_text),
            expected.first().cloned()
        );
        assert_eq!(
            first.state().last_key().map(PicX::to_text),
            expected.last().cloned()
        );
        let PageRows::Read(slots) = first.rows() else {
            return Err("COTRN00C-014: the rows were kept".into());
        };
        let row = slots
            .first()
            .and_then(Option::as_ref)
            .ok_or("no first row")?;
        assert_eq!(row.tran_id().as_bytes(), b"0000000000683580");
        assert_eq!(row.display_date().map(PicX::as_bytes), Some(b"06/10/22"));
        assert_eq!(row.date(), Some(date(2022, 6, 10)?));
        assert_eq!(row.description().as_bytes(), b"Purchase at Abshire-Lowe  ");
        assert_eq!(row.amount(), "+00000504.77");
        assert!(
            !format!("{first:?}").contains("4859452612877065"),
            "COTRN00C-034"
        );
        Ok(())
    }

    // audit: COTRN00C-015
    // audit: COTRN00C-017
    // audit: COTRN00C-018
    // audit: COTRN00C-021
    // audit: COTRN00C-022
    #[test]
    fn paging_keys_move_the_page_and_answer_at_the_edges() -> TestResult {
        let all = transactions()?;
        let first_browse = browse_of(plan(ListKey::Enter, None, TranListState::default(), 10)?)?;
        let first = page(first_browse.clone(), read(&first_browse, &all))?;
        let next = browse_of(plan(ListKey::Pf8, None, first.state().clone(), 10)?)?;
        assert_eq!(next.direction(), Direction::Forward);
        assert_eq!(next.limit(), 12);
        let second = page(next.clone(), read(&next, &all))?;
        let expected: Vec<String> = all
            .iter()
            .skip(10)
            .take(10)
            .map(|t| t.id.to_text())
            .collect();
        assert_eq!(shown_ids(&second)?, expected);
        assert_eq!(second.state().page_no(), PicU::new(2)?);
        // The last 5 of the 300 records: a short page reaches the bottom.
        let tail = &all[290..];
        let near_end = browse_of(plan(
            ListKey::Enter,
            Some(&tail[0].id.to_text()),
            TranListState::default(),
            10,
        )?)?;
        let last = page(near_end.clone(), read(&near_end, &all))?;
        assert_eq!(last.message(), Some(REACHED_BOTTOM));
        assert_eq!(
            last.message(),
            Some("You have reached the bottom of the page...")
        );
        assert_eq!(last.state().next_page(), NextPage::No);
        let bottom = answer_of(plan(ListKey::Pf8, None, last.state().clone(), 10)?)?;
        assert_eq!(bottom.message(), Some(ALREADY_AT_BOTTOM));
        assert_eq!(bottom.rows(), &PageRows::Kept);
        let top = answer_of(plan(ListKey::Pf7, None, first.state().clone(), 10)?)?;
        assert_eq!(top.message(), Some(ALREADY_AT_TOP));
        assert_eq!(
            top.message(),
            Some("You are already at the top of the page...")
        );
        assert_eq!(top.state().next_page(), NextPage::Yes);
        Ok(())
    }

    // audit: COTRN00C-023
    // audit: COTRN00C-024
    // audit: COTRN00C-025
    #[test]
    fn f7_reads_the_page_before_the_first_key() -> TestResult {
        let all = transactions()?;
        let mut shown = page(
            browse_of(plan(ListKey::Enter, None, TranListState::default(), 10)?)?,
            all.iter().take(11).map(row_from).collect(),
        )?;
        for _ in 0..2 {
            let next = browse_of(plan(ListKey::Pf8, None, shown.state().clone(), 10)?)?;
            shown = page(next.clone(), read(&next, &all))?;
        }
        assert_eq!(shown.state().page_no(), PicU::new(3)?);
        let back = browse_of(plan(ListKey::Pf7, None, shown.state().clone(), 10)?)?;
        assert_eq!(back.direction(), Direction::Backward);
        let second = page(back.clone(), read(&back, &all))?;
        let expected: Vec<String> = all
            .iter()
            .skip(10)
            .take(10)
            .map(|t| t.id.to_text())
            .collect();
        assert_eq!(shown_ids(&second)?, expected);
        assert_eq!(second.state().page_no(), PicU::new(2)?);
        assert_eq!(second.message(), None);
        let back = browse_of(plan(ListKey::Pf7, None, second.state().clone(), 10)?)?;
        let first = page(back.clone(), read(&back, &all))?;
        assert_eq!(
            first.message(),
            Some("You have reached the top of the page...")
        );
        assert_eq!(first.state().page_no(), PicU::new(1)?);
        Ok(())
    }
}

mod cotrn01c {
    use domain::online::cotrn01c::{
        FailureClass, TranLookup, TranViewFailure, TranViewField, edit_tran_id, view,
    };
    use domain::records::TransactionRecord;

    use super::{TestResult, at, date, fixture};

    // audit: COTRN01C-009
    // audit: COTRN01C-010
    // audit: COTRN01C-012
    // audit: COTRN01C-013
    #[test]
    fn the_key_edit_and_the_failed_reads_give_their_messages() -> TestResult {
        for blank in ["", "                ", "\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0"] {
            assert_eq!(edit_tran_id(blank).err(), Some(TranViewFailure::Blank));
        }
        assert_eq!(edit_tran_id(" 683580")?.as_bytes(), b" 683580         ");
        for (failure, message, status) in [
            (TranViewFailure::Blank, "Tran ID can NOT be empty...", 400),
            (
                TranViewFailure::NotFound,
                "Transaction ID NOT found...",
                404,
            ),
            (
                TranViewFailure::LookupFailed,
                "Unable to lookup Transaction...",
                500,
            ),
        ] {
            assert_eq!(failure.message(), message);
            assert_eq!(failure.field(), TranViewField::TranId);
            assert_eq!(failure.field().name(), "tran_id");
            assert_eq!(failure.class().http_status(), status);
        }
        assert_eq!(
            view(TranLookup::NotFound).err(),
            Some(TranViewFailure::NotFound)
        );
        assert_eq!(
            view(TranLookup::Error).err().map(TranViewFailure::class),
            Some(FailureClass::Lookup)
        );
        Ok(())
    }

    // audit: COTRN01C-015
    // audit: COTRN01C-016
    // audit: COTRN01C-017
    // audit: COTRN01C-018
    // audit: COTRN01C-019
    // audit: COTRN01C-020
    // audit: COTRN01C-021
    // audit: COTRN01C-022
    // audit: COTRN01C-023
    // audit: COTRN01C-024
    // audit: COTRN01C-025
    // audit: COTRN01C-026
    // audit: COTRN01C-027
    // audit: COTRN01C-028
    #[test]
    fn a_found_transaction_shows_every_detail_field() -> TestResult {
        let record = fixture::<TransactionRecord>("dailytran.txt")?
            .into_iter()
            .next()
            .ok_or("no fixture")?;
        let shown = view(TranLookup::Found(&record))?;
        assert_eq!(shown.tran_id.as_bytes(), b"0000000000683580");
        assert_eq!(shown.card_num.to_string(), "************7065");
        assert!(
            !format!("{shown:?}").contains("4859452612877065"),
            "COTRN01C-017"
        );
        assert_eq!(shown.type_cd.as_bytes(), b"01");
        assert_eq!(shown.cat_cd.as_bytes(), b"0001");
        assert_eq!(shown.source.to_text(), "POS TERM");
        assert_eq!(shown.desc.to_text(), "Purchase at Abshire-Lowe");
        assert_eq!(shown.desc.as_bytes().len(), 60);
        assert_eq!(shown.amount, "+00000504.77");
        assert_eq!(shown.orig_ts, Some(at(date(2022, 6, 10)?, 19, 27, 53)?));
        assert_eq!(shown.proc_ts, None);
        assert_eq!(shown.merchant_id.as_bytes(), b"800000000");
        assert_eq!(shown.merchant_name.to_text(), "Abshire-Lowe");
        assert_eq!(shown.merchant_city.to_text(), "North Enoshaven");
        assert_eq!(shown.merchant_zip.as_bytes(), b"72112     ");
        Ok(())
    }
}

mod cotrn02c {
    use chrono::NaiveDateTime;
    use domain::copybook::{CobolTimestamp, PicTimestamp, PicU, PicX, TsForm};
    use domain::money::{Decimal, Money};
    use domain::online::cotrn02c::{
        AddFailure, AddField, AddOutcome, Confirmation, FailureClass, KeyRequest, TranAddInput,
        XrefLookup, build_record, confirm, copy_last, edit_data_with, edit_keys, fill_key,
        success_message,
    };
    use domain::online::csutldtc::{DateMask, LeFeedback, LeResult};
    use domain::records::{CardXrefRecord, TransactionRecord};

    use super::{TestResult, at, date, fixture};

    const VALID: TranAddInput<'static> = TranAddInput {
        type_cd: "01",
        cat_cd: "0001",
        source: "POS TERM",
        description: "Purchase at Abshire-Lowe",
        amount: "+00000012.34",
        orig_date: "2022-06-10",
        proc_date: "2022-06-11",
        merchant_id: "800000000",
        merchant_name: "Abshire-Lowe",
        merchant_city: "North Enoshaven",
        merchant_zip: "72112",
        confirm: "Y",
    };

    fn accept_all(text: &str, mask: DateMask) -> LeResult {
        LeResult::new(text, mask, LeFeedback::Valid)
    }

    fn first_xref() -> Result<CardXrefRecord, Box<dyn std::error::Error>> {
        Ok(fixture::<CardXrefRecord>("cardxref.txt")?
            .into_iter()
            .next()
            .ok_or("no fixture")?)
    }

    fn failure_of(failure: Option<AddFailure>) -> Option<(&'static str, Option<AddField>, u16)> {
        failure.map(|f| (f.message(), f.field(), f.class().http_status()))
    }

    /// [`VALID`] with the fields `fields` left empty.
    fn blanked(fields: &[AddField]) -> TranAddInput<'static> {
        let mut input = VALID;
        for field in fields {
            let value = match field {
                AddField::TypeCd => &mut input.type_cd,
                AddField::CatCd => &mut input.cat_cd,
                AddField::Source => &mut input.source,
                AddField::Description => &mut input.description,
                AddField::Amount => &mut input.amount,
                AddField::OrigDate => &mut input.orig_date,
                AddField::ProcDate => &mut input.proc_date,
                AddField::MerchantId => &mut input.merchant_id,
                AddField::MerchantName => &mut input.merchant_name,
                AddField::MerchantCity => &mut input.merchant_city,
                AddField::MerchantZip => &mut input.merchant_zip,
                AddField::Confirm => &mut input.confirm,
                // The key fields travel apart from the data fields, in `KeyRequest`.
                AddField::AcctId | AddField::CardNum => continue,
            };
            *value = "";
        }
        input
    }

    // audit: COTRN02C-007
    #[test]
    fn an_unserved_key_gets_the_whole_invalid_key_message_on_no_field() {
        let refused = AddFailure::invalid_key();
        let expected = format!("{:<50}", "Invalid key pressed. Please see below...");
        assert_eq!(refused.message(), expected);
        assert_eq!(refused.message().len(), 50);
        assert_eq!(refused.to_string(), expected);
        assert_eq!(refused.class(), FailureClass::Input);
        assert_eq!(
            failure_of(Some(refused)),
            Some((expected.as_str(), None, 400))
        );
    }

    // audit: COTRN02C-009
    // audit: COTRN02C-010
    // audit: COTRN02C-011
    // audit: COTRN02C-015
    // audit: COTRN02C-016
    #[test]
    fn the_key_edits_choose_the_account_or_the_card_path() -> TestResult {
        let cases = [
            (
                None,
                None,
                "Account or Card Number must be entered...",
                AddField::AcctId,
            ),
            (
                Some(""),
                Some("     "),
                "Account or Card Number must be entered...",
                AddField::AcctId,
            ),
            (
                Some("50"),
                Some("0500024453765740"),
                "Account ID must be Numeric...",
                AddField::AcctId,
            ),
            (
                None,
                Some("05000244537657"),
                "Card Number must be Numeric...",
                AddField::CardNum,
            ),
        ];
        for (acct, card, message, field) in cases {
            assert_eq!(
                failure_of(edit_keys(acct, card).err()),
                Some((message, Some(field), 400))
            );
        }
        let by_account = edit_keys(Some("00000000050"), Some("not a card"))?;
        assert_eq!(by_account, KeyRequest::Account(PicU::new(50)?));
        let by_card = edit_keys(None, Some("0500024453765740"))?;
        assert!(
            matches!(by_card, KeyRequest::Card(ref number) if number.redacted() == "************5740"),
            "COTRN02C-016"
        );
        Ok(())
    }

    // audit: COTRN02C-012
    // audit: COTRN02C-013
    // audit: COTRN02C-014
    // audit: COTRN02C-017
    // audit: COTRN02C-018
    // audit: COTRN02C-019
    #[test]
    fn the_cross_reference_fills_the_other_key() -> TestResult {
        let xref = first_xref()?;
        let filled = fill_key(
            KeyRequest::Account(PicU::new(50)?),
            XrefLookup::Found(&xref),
        )?;
        assert_eq!(filled.acct_id().get(), 50);
        assert_eq!(filled.card_num().to_string(), "************5740");
        let from_card = fill_key(
            edit_keys(None, Some("0500024453765740"))?,
            XrefLookup::Found(&xref),
        )?;
        assert_eq!(from_card.acct_id().get(), 50);
        let account_request = KeyRequest::Account(PicU::new(50)?);
        let card_request = edit_keys(None, Some("0500024453765740"))?;
        for (request, lookup, message, field, status) in [
            (
                account_request.clone(),
                XrefLookup::NotFound,
                "Account ID NOT found...",
                AddField::AcctId,
                404,
            ),
            (
                account_request,
                XrefLookup::Error,
                "Unable to lookup Acct in XREF AIX file...",
                AddField::AcctId,
                500,
            ),
            (
                card_request.clone(),
                XrefLookup::NotFound,
                "Card Number NOT found...",
                AddField::CardNum,
                404,
            ),
            (
                card_request,
                XrefLookup::Error,
                "Unable to lookup Card # in XREF file...",
                AddField::CardNum,
                500,
            ),
        ] {
            assert_eq!(
                failure_of(fill_key(request, lookup).err()),
                Some((message, Some(field), status))
            );
        }
        Ok(())
    }

    // audit: COTRN02C-020
    // audit: COTRN02C-021
    // audit: COTRN02C-022
    // audit: COTRN02C-023
    // audit: COTRN02C-024
    // audit: COTRN02C-025
    // audit: COTRN02C-026
    // audit: COTRN02C-027
    // audit: COTRN02C-028
    // audit: COTRN02C-029
    // audit: COTRN02C-030
    #[test]
    fn each_empty_data_field_is_reported_in_screen_order() {
        let cases = [
            (AddField::TypeCd, "Type CD"),
            (AddField::CatCd, "Category CD"),
            (AddField::Source, "Source"),
            (AddField::Description, "Description"),
            (AddField::Amount, "Amount"),
            (AddField::OrigDate, "Orig Date"),
            (AddField::ProcDate, "Proc Date"),
            (AddField::MerchantId, "Merchant ID"),
            (AddField::MerchantName, "Merchant Name"),
            (AddField::MerchantCity, "Merchant City"),
            (AddField::MerchantZip, "Merchant Zip"),
        ];
        for (field, label) in cases {
            let message = format!("{label} can NOT be empty...");
            let failure = failure_of(edit_data_with(&blanked(&[field]), accept_all).err());
            assert_eq!(
                failure.map(|(text, at, status)| (text.to_owned(), at, status)),
                Some((message, Some(field), 400))
            );
        }
        let both = failure_of(
            edit_data_with(
                &blanked(&[AddField::MerchantZip, AddField::TypeCd]),
                accept_all,
            )
            .err(),
        );
        assert_eq!(
            both,
            Some(("Type CD can NOT be empty...", Some(AddField::TypeCd), 400))
        );
    }

    // audit: COTRN02C-031
    // audit: COTRN02C-032
    // audit: COTRN02C-033
    // audit: COTRN02C-034
    // audit: COTRN02C-035
    // audit: COTRN02C-036
    // audit: COTRN02C-039
    #[test]
    fn the_format_edits_follow_the_empty_checks() -> TestResult {
        let cases = [
            (
                TranAddInput {
                    type_cd: "1",
                    ..VALID
                },
                "Type CD must be Numeric...",
                AddField::TypeCd,
            ),
            (
                TranAddInput {
                    cat_cd: "5",
                    ..VALID
                },
                "Category CD must be Numeric...",
                AddField::CatCd,
            ),
            (
                TranAddInput {
                    amount: "12.34",
                    ..VALID
                },
                "Amount should be in format -99999999.99",
                AddField::Amount,
            ),
            (
                TranAddInput {
                    amount: "+0000001234",
                    ..VALID
                },
                "Amount should be in format -99999999.99",
                AddField::Amount,
            ),
            (
                TranAddInput {
                    orig_date: "2022/06/10",
                    ..VALID
                },
                "Orig Date should be in format YYYY-MM-DD",
                AddField::OrigDate,
            ),
            (
                TranAddInput {
                    proc_date: "06-11-2022",
                    ..VALID
                },
                "Proc Date should be in format YYYY-MM-DD",
                AddField::ProcDate,
            ),
            // With both dates accepted, the merchant ID edit runs last.
            (
                TranAddInput {
                    merchant_id: "80000000A",
                    ..VALID
                },
                "Merchant ID must be Numeric...",
                AddField::MerchantId,
            ),
        ];
        for (input, message, field) in cases {
            assert_eq!(
                failure_of(edit_data_with(&input, accept_all).err()),
                Some((message, Some(field), 400))
            );
        }
        let fields = edit_data_with(&VALID, accept_all)?;
        assert_eq!(fields.type_cd().as_bytes(), b"01");
        assert_eq!(fields.cat_cd().as_bytes(), b"0001");
        assert_eq!(fields.amount_text(), "+00000012.34");
        assert_eq!(fields.orig_date(), date(2022, 6, 10)?);
        assert_eq!(fields.proc_date(), date(2022, 6, 11)?);
        assert_eq!(fields.merchant_id().as_bytes(), b"800000000");
        let negative = edit_data_with(
            &TranAddInput {
                amount: "-00000000.50",
                ..VALID
            },
            accept_all,
        )?;
        assert_eq!(negative.amount_text(), "-00000000.50");
        Ok(())
    }

    // audit: COTRN02C-040
    // audit: COTRN02C-041
    // audit: COTRN02C-042
    #[test]
    fn the_confirmation_prompts_adds_or_is_refused() -> TestResult {
        for (raw, expected) in [
            ("Y", Confirmation::Yes),
            ("y", Confirmation::Yes),
            ("N", Confirmation::NotConfirmed),
            ("n", Confirmation::NotConfirmed),
            ("", Confirmation::NotConfirmed),
            ("X", Confirmation::Invalid),
        ] {
            assert_eq!(confirm(raw), expected, "{raw:?}");
        }
        let refused = AddFailure::invalid_confirmation();
        assert_eq!(
            failure_of(Some(refused)),
            Some((
                "Invalid value. Valid values are (Y/N)...",
                Some(AddField::Confirm),
                400
            ))
        );
        let key = fill_key(
            KeyRequest::Account(PicU::new(50)?),
            XrefLookup::Found(&first_xref()?),
        )?;
        let prompt = AddOutcome::prompt(key, edit_data_with(&VALID, accept_all)?, None);
        assert_eq!(prompt.message(), "Confirm to add this transaction...");
        assert_eq!(prompt.focus(), AddField::Confirm);
        Ok(())
    }

    // audit: COTRN02C-045
    // audit: COTRN02C-046
    // audit: COTRN02C-047
    // audit: COTRN02C-049
    // audit: COTRN02C-050
    // audit: COTRN02C-051
    #[test]
    fn the_added_record_holds_the_screen_values_in_layout_s() -> TestResult {
        let key = fill_key(
            KeyRequest::Account(PicU::new(50)?),
            XrefLookup::Found(&first_xref()?),
        )?;
        let fields = edit_data_with(&VALID, accept_all)?;
        let new_id = PicX::from_text("0000000996722788")?;
        let record = build_record(new_id.clone(), &key, &fields, PicX::spaces());
        assert_eq!(record.id, new_id);
        assert_eq!(record.type_cd.as_bytes(), b"01");
        assert_eq!(record.cat_cd.get(), 1);
        assert_eq!(record.source.to_text(), "POS TERM");
        assert_eq!(record.desc.to_text(), "Purchase at Abshire-Lowe");
        assert_eq!(record.amt.as_decimal().to_string(), "12.34");
        assert_eq!(record.merchant_id.get(), 800_000_000);
        assert!(record.card_num == *key.card_num(), "COTRN02C-045");
        for (stamp, day) in [(record.orig_ts.value(), 10), (record.proc_ts.value(), 11)] {
            let stamp = stamp.ok_or("COTRN02C-046: a blank timestamp")?;
            assert_eq!(stamp.form(), TsForm::ScreenDate);
            assert_eq!(stamp.value(), at(date(2022, 6, day)?, 0, 0, 0)?);
        }
        let message = success_message(&new_id);
        assert_eq!(
            message,
            "Transaction added successfully.  Your Tran ID is 0000000996722788."
        );
        assert_eq!(message.len(), 66);
        assert_eq!(AddOutcome::added(new_id.clone()).message(), message);
        for (failure, text) in [
            (AddFailure::duplicate_id(), "Tran ID already exist..."),
            (AddFailure::write_failed(), "Unable to Add Transaction..."),
            (
                AddFailure::transaction_lookup(),
                "Unable to lookup Transaction...",
            ),
        ] {
            assert_eq!(
                (failure.message(), failure.field(), failure.class()),
                (text, Some(AddField::AcctId), FailureClass::Store)
            );
        }
        Ok(())
    }

    /// `dailytran.txt:300`, the fixture's last record: type `01`, category 1, source `POS TERM`,
    /// description `Purchase at Kilback LLC`, amount `0000006032B` (+603.22, FIXPOST-300),
    /// merchant 800000000 `Kilback LLC` of `Cummeratamouth` `53200-7529`, original timestamp
    /// `2022-06-10 19:27:53.000000` in layout `I` and a blank processing timestamp.
    fn last_transaction() -> Result<TransactionRecord, Box<dyn std::error::Error>> {
        Ok(fixture::<TransactionRecord>("dailytran.txt")?
            .into_iter()
            .last()
            .ok_or("no fixture")?)
    }

    /// `count` ten-character blocks, each `lead`, the block's index and `abcdefgh`
    /// (`D0abcdefghD1abcdefgh`...), so a cut at another offset, or a field copied from another
    /// field, shows.
    fn blocks(lead: char, count: usize) -> String {
        "0123456789"
            .chars()
            .take(count)
            .flat_map(|index| [lead, index].into_iter().chain("abcdefgh".chars()))
            .collect()
    }

    /// A non-blank `PIC X(26)` timestamp holding `value` in layout `form`.
    fn stamp(
        value: NaiveDateTime,
        form: TsForm,
    ) -> Result<PicTimestamp, Box<dyn std::error::Error>> {
        Ok(PicTimestamp::new(Some(CobolTimestamp::new(value, form)?)))
    }

    // audit: COTRN02C-054
    // audit: COTRN02C-055
    #[test]
    fn f5_copies_the_last_transaction_into_the_screen_fields() -> TestResult {
        let copied = copy_last(&last_transaction()?);
        let description = format!("{:<60}", "Purchase at Kilback LLC");
        let merchant_name = format!("{:<30}", "Kilback LLC");
        let merchant_city = format!("{:<25}", "Cummeratamouth");
        let expected = TranAddInput {
            type_cd: "01",
            cat_cd: "0001",
            source: "POS TERM  ",
            description: &description,
            amount: "+00000603.22",
            orig_date: "2022-06-10",
            proc_date: "          ",
            merchant_id: "800000000",
            merchant_name: &merchant_name,
            merchant_city: &merchant_city,
            merchant_zip: "53200-7529",
            confirm: "Y",
        };
        assert_eq!(copied.as_input("Y"), expected);
        assert_eq!(copied.orig_date(), Some(date(2022, 6, 10)?));
        assert_eq!(copied.proc_date(), None);
        Ok(())
    }

    // audit: COTRN02C-054
    // audit: COTRN02C-055
    #[test]
    fn f5_cuts_each_copied_field_and_amount_to_its_map_picture() -> TestResult {
        let mut last = last_transaction()?;
        last.type_cd = PicX::from_text("02")?;
        last.cat_cd = PicU::new(5)?;
        last.source = PicX::from_text("WEB PORTAL")?;
        last.desc = PicX::from_text(&blocks('D', 10))?;
        last.merchant_id = PicU::new(42)?;
        last.merchant_name = PicX::from_text(&blocks('N', 5))?;
        last.merchant_city = PicX::from_text(&blocks('C', 5))?;
        last.merchant_zip = PicX::from_text("K1A 0B1")?;
        let copied = copy_last(&last);
        let description = blocks('D', 6);
        let merchant_name = blocks('N', 3);
        let merchant_city = format!("{}C2abc", blocks('C', 2));
        let expected = TranAddInput {
            type_cd: "02",
            cat_cd: "0005",
            source: "WEB PORTAL",
            description: &description,
            amount: "+00000603.22",
            orig_date: "2022-06-10",
            proc_date: "          ",
            merchant_id: "000000042",
            merchant_name: &merchant_name,
            merchant_city: &merchant_city,
            merchant_zip: "K1A 0B1   ",
            confirm: "N",
        };
        assert_eq!(copied.as_input("N"), expected);
        // No negative amount whose eight kept integer digits and two decimals are all zero:
        // the sign of that edit is the unresolved negative-zero case X-ARITH-7.
        for (cents, edited) in [
            (0, "+00000000.00"),
            (1, "+00000000.01"),
            (-1, "-00000000.01"),
            (-1_234, "-00000012.34"),
            (9_999_999_999, "+99999999.99"),
            (-9_999_999_999, "-99999999.99"),
            (10_000_000_000, "+00000000.00"),
            (12_345_678_901, "+23456789.01"),
            (-12_345_678_901, "-23456789.01"),
            (99_999_999_999, "+99999999.99"),
        ] {
            last.amt = Money::new(Decimal::new(cents, 2))?;
            assert_eq!(copy_last(&last).amount(), edited, "{cents} cents");
        }
        let stamps = [
            (
                stamp(at(date(2023, 1, 2)?, 3, 4, 5)?, TsForm::Db2)?,
                "2023-01-02",
                Some(date(2023, 1, 2)?),
            ),
            (
                stamp(at(date(2021, 12, 31)?, 0, 0, 0)?, TsForm::ScreenDate)?,
                "2021-12-31",
                Some(date(2021, 12, 31)?),
            ),
            (
                stamp(at(date(2024, 2, 29)?, 23, 59, 59)?, TsForm::Iso)?,
                "2024-02-29",
                Some(date(2024, 2, 29)?),
            ),
            (PicTimestamp::BLANK, "          ", None),
        ];
        // Each layout in `TRAN-ORIG-TS` beside the next one in `TRAN-PROC-TS`.
        for (orig, proc) in stamps.iter().zip(stamps.iter().cycle().skip(1)) {
            (last.orig_ts, last.proc_ts) = (orig.0, proc.0);
            let copied = copy_last(&last);
            assert_eq!(
                (copied.orig_date_text(), copied.orig_date()),
                (orig.1, orig.2)
            );
            assert_eq!(
                (copied.proc_date_text(), copied.proc_date()),
                (proc.1, proc.2)
            );
        }
        Ok(())
    }
}

mod corpt00c {
    use domain::online::corpt00c::{
        ConfirmOutcome, DECK_RECORD_COUNT, DECK_RECORD_LEN, FailureClass, ReportField, ReportInput,
        ReportKind, ReportRequest, confirm, date_window, edit, edit_with, job_deck,
        submitted_message, tdq_write_failure,
    };
    use domain::online::csutldtc::{DateMask, LeFeedback, LeResult};

    use super::{TestResult, at, clock, date};

    type Outcome<T> = Result<T, Box<dyn std::error::Error>>;

    const CUSTOM: ReportInput<'static> = ReportInput {
        monthly: "",
        yearly: "",
        custom: "X",
        start_month: "1",
        start_day: "5",
        start_year: "2022",
        end_month: "07",
        end_day: "06",
        end_year: "2022",
        confirm: "",
    };

    fn accept_all(text: &str, mask: DateMask) -> LeResult {
        LeResult::new(text, mask, LeFeedback::Valid)
    }

    fn custom(input: &ReportInput<'_>) -> Outcome<ReportRequest> {
        Ok(edit_with(input, &clock()?, accept_all)?)
    }

    fn input_failure(input: &ReportInput<'_>) -> Outcome<(String, ReportField, u16)> {
        let failure = edit_with(input, &clock()?, accept_all)
            .err()
            .ok_or("the edits passed")?;
        Ok((
            failure.message().to_owned(),
            failure.field(),
            failure.class().http_status(),
        ))
    }

    // audit: CORPT00C-007
    // audit: CORPT00C-008
    // audit: CORPT00C-009
    // audit: CORPT00C-010
    // audit: CORPT00C-029
    #[test]
    fn the_first_selected_report_takes_its_clock_window() -> TestResult {
        let clock = clock()?;
        let july = (date(2022, 7, 1)?, date(2022, 7, 31)?);
        assert_eq!(date_window(ReportKind::Monthly, &clock), Some(july));
        let year = (date(2022, 1, 1)?, date(2022, 12, 31)?);
        assert_eq!(date_window(ReportKind::Yearly, &clock), Some(year));
        assert_eq!(date_window(ReportKind::Custom, &clock), None);
        let leap = domain::FixedClock::new(at(date(2024, 2, 10)?, 8, 0, 0)?);
        assert_eq!(
            date_window(ReportKind::Monthly, &leap).map(|(_, end)| end),
            Some(date(2024, 2, 29)?)
        );
        let monthly = edit(
            &ReportInput {
                monthly: "N",
                yearly: "X",
                ..ReportInput::default()
            },
            &clock,
        )?;
        assert_eq!(
            (monthly.kind(), monthly.start(), monthly.end()),
            (ReportKind::Monthly, july.0, july.1)
        );
        let yearly = edit(
            &ReportInput {
                yearly: "X",
                custom: "X",
                ..ReportInput::default()
            },
            &clock,
        )?;
        assert_eq!(
            (yearly.kind(), yearly.start(), yearly.end()),
            (ReportKind::Yearly, year.0, year.1)
        );
        let none = edit(&ReportInput::default(), &clock)
            .err()
            .ok_or("no selection passed")?;
        assert_eq!(none.message(), "Select a report type to print report...");
        assert_eq!(
            (none.field(), none.class()),
            (ReportField::Monthly, FailureClass::Input)
        );
        Ok(())
    }

    // audit: CORPT00C-011
    // audit: CORPT00C-012
    // audit: CORPT00C-013
    // audit: CORPT00C-014
    // audit: CORPT00C-015
    // audit: CORPT00C-016
    #[test]
    fn each_empty_custom_part_is_reported_in_order() -> TestResult {
        let cases = [
            (
                ReportInput {
                    start_month: "",
                    start_day: "",
                    ..CUSTOM
                },
                "Start Date - Month can NOT be empty...",
                ReportField::StartMonth,
            ),
            (
                ReportInput {
                    start_day: "",
                    ..CUSTOM
                },
                "Start Date - Day can NOT be empty...",
                ReportField::StartDay,
            ),
            (
                ReportInput {
                    start_year: "",
                    ..CUSTOM
                },
                "Start Date - Year can NOT be empty...",
                ReportField::StartYear,
            ),
            (
                ReportInput {
                    end_month: "",
                    ..CUSTOM
                },
                "End Date - Month can NOT be empty...",
                ReportField::EndMonth,
            ),
            (
                ReportInput {
                    end_day: "",
                    ..CUSTOM
                },
                "End Date - Day can NOT be empty...",
                ReportField::EndDay,
            ),
            (
                ReportInput {
                    end_year: "",
                    ..CUSTOM
                },
                "End Date - Year can NOT be empty...",
                ReportField::EndYear,
            ),
        ];
        for (input, message, field) in cases {
            assert_eq!(input_failure(&input)?, (message.to_owned(), field, 400));
        }
        Ok(())
    }

    // audit: CORPT00C-017
    // audit: CORPT00C-018
    // audit: CORPT00C-019
    // audit: CORPT00C-020
    // audit: CORPT00C-021
    // audit: CORPT00C-022
    // audit: CORPT00C-023
    // audit: CORPT00C-024
    // audit: CORPT00C-025
    // audit: CORPT00C-026
    // audit: CORPT00C-028
    #[test]
    fn custom_parts_are_normalised_and_range_checked() -> TestResult {
        let cases = [
            (
                ReportInput {
                    start_month: "13",
                    ..CUSTOM
                },
                "Start Date - Not a valid Month...",
                ReportField::StartMonth,
            ),
            (
                ReportInput {
                    start_day: "32",
                    ..CUSTOM
                },
                "Start Date - Not a valid Day...",
                ReportField::StartDay,
            ),
            (
                ReportInput {
                    end_month: "13",
                    ..CUSTOM
                },
                "End Date - Not a valid Month...",
                ReportField::EndMonth,
            ),
            (
                ReportInput {
                    end_day: "99",
                    ..CUSTOM
                },
                "End Date - Not a valid Day...",
                ReportField::EndDay,
            ),
        ];
        for (input, message, field) in cases {
            assert_eq!(input_failure(&input)?, (message.to_owned(), field, 400));
        }
        let request = custom(&CUSTOM)?;
        assert_eq!(request.kind(), ReportKind::Custom);
        assert_eq!(
            (request.start(), request.end()),
            (date(2022, 1, 5)?, date(2022, 7, 6)?)
        );
        // A year is never rejected by the part edits: `22` stands for year 0022.
        let early = custom(&ReportInput {
            start_year: "22",
            ..CUSTOM
        })?;
        assert_eq!(early.start(), date(22, 1, 5)?);
        let padded = custom(&ReportInput {
            start_day: " 5 ",
            end_year: "9",
            ..CUSTOM
        })?;
        assert_eq!(
            (padded.start(), padded.end()),
            (date(2022, 1, 5)?, date(9, 7, 6)?)
        );
        // The verdict of CSUTLDTC is substituted here; which dates it rejects is HALTED(H5).
        let reject_start = |text: &str, mask: DateMask| {
            let feedback = if text.starts_with("2022-01") {
                LeFeedback::BadDateValue
            } else {
                LeFeedback::Valid
            };
            LeResult::new(text, mask, feedback)
        };
        let start = edit_with(&CUSTOM, &clock()?, reject_start)
            .err()
            .ok_or("a rejected start passed")?;
        assert_eq!(
            (start.message(), start.field()),
            ("Start Date - Not a valid date...", ReportField::StartMonth)
        );
        let reject_end = |text: &str, mask: DateMask| {
            let feedback = if text.starts_with("2022-07") {
                LeFeedback::BadDateValue
            } else {
                LeFeedback::Valid
            };
            LeResult::new(text, mask, feedback)
        };
        let end = edit_with(&CUSTOM, &clock()?, reject_end)
            .err()
            .ok_or("a rejected end passed")?;
        assert_eq!(
            (end.message(), end.field()),
            ("End Date - Not a valid date...", ReportField::EndMonth)
        );
        Ok(())
    }

    // audit: CORPT00C-030
    // audit: CORPT00C-031
    // audit: CORPT00C-032
    // audit: CORPT00C-033
    // audit: CORPT00C-037
    // audit: CORPT00C-038
    #[test]
    fn the_confirmation_submits_clears_or_is_refused() -> TestResult {
        let request = edit(
            &ReportInput {
                monthly: "X",
                ..ReportInput::default()
            },
            &clock()?,
        )?;
        let blank = confirm(request, "")
            .err()
            .ok_or("a blank confirmation passed")?;
        assert_eq!(
            blank.message(),
            "Please confirm to print the Monthly report..."
        );
        assert_eq!(blank.field(), ReportField::Confirm);
        assert_eq!(confirm(request, "Y")?, ConfirmOutcome::Submit(request));
        assert_eq!(confirm(request, "y")?, ConfirmOutcome::Submit(request));
        assert_eq!(confirm(request, "n")?, ConfirmOutcome::Cleared);
        let invalid = confirm(request, "X").err().ok_or("X passed")?;
        assert_eq!(
            invalid.message(),
            "\"X\" is not a valid value to confirm..."
        );
        assert_eq!(invalid.message().len(), 38);
        assert_eq!(invalid.class().http_status(), 400);
        for (kind, message) in [
            (
                ReportKind::Monthly,
                "Monthly report submitted for printing ...",
            ),
            (
                ReportKind::Yearly,
                "Yearly report submitted for printing ...",
            ),
            (
                ReportKind::Custom,
                "Custom report submitted for printing ...",
            ),
        ] {
            assert_eq!(submitted_message(kind), message);
        }
        let write = tdq_write_failure();
        assert_eq!(
            (write.message(), write.field()),
            ("Unable to Write TDQ (JOBS)...", ReportField::Monthly)
        );
        assert_eq!(write.class().http_status(), 500);
        Ok(())
    }

    // audit: CORPT00C-034
    // audit: CORPT00C-035
    #[test]
    fn the_job_deck_is_seventeen_80_byte_records_with_the_dates() -> TestResult {
        let request = custom(&CUSTOM)?;
        let deck = job_deck(&request);
        assert_eq!(
            (deck.len(), DECK_RECORD_COUNT, DECK_RECORD_LEN),
            (17, 17, 80)
        );
        let expected = [
            "//TRNRPT00 JOB 'TRAN REPORT',CLASS=A,MSGCLASS=0,",
            "// NOTIFY=&SYSUID",
            "//*",
            "//JOBLIB JCLLIB ORDER=('AWS.M2.CARDDEMO.PROC')",
            "//*",
            "//STEP10 EXEC PROC=TRANREPT",
            "//*",
            "//STEP05R.SYMNAMES DD *",
            "TRAN-CARD-NUM,263,16,ZD",
            "TRAN-PROC-DT,305,10,CH",
            "PARM-START-DATE,C'2022-01-05'",
            "PARM-END-DATE,C'2022-07-06'",
            "/*",
            "//STEP10R.DATEPARM DD *",
            "2022-01-05 2022-07-06",
            "/*",
            "/*EOF",
        ];
        for (record, text) in deck.iter().zip(expected) {
            assert_eq!(record.as_bytes().len(), 80);
            assert_eq!(record.to_text(), text);
            assert_eq!(
                record
                    .as_bytes()
                    .get(text.len()..)
                    .map(|tail| tail.iter().all(|&b| b == b' ')),
                Some(true)
            );
        }
        Ok(())
    }
}

mod cobil00c {
    use domain::copybook::{PicU, PicX, TsForm};
    use domain::online::cobil00c::{
        AccountKey, AccountRewrite, BillPayDecision, BillPayField, BillPayInput, BillPayOutcome,
        Confirm, EnterAction, ReadIntent, TranWrite, account_lookup_failed, account_not_found,
        balance_display, confirm, decide, edit, edit_account, payment, payment_outcome,
        success_message,
    };
    use domain::records::{AccountRecord, CardXrefRecord};

    use super::{TestResult, clock, fixture, has_layout};

    type Outcome<T> = Result<T, Box<dyn std::error::Error>>;

    fn account(id: u64) -> Outcome<AccountRecord> {
        Ok(fixture::<AccountRecord>("acctdata.txt")?
            .into_iter()
            .find(|record| record.id.get() == id)
            .ok_or("no such fixture account")?)
    }

    // audit: COBIL00C-007
    // audit: COBIL00C-008
    // audit: COBIL00C-009
    // audit: COBIL00C-010
    // audit: COBIL00C-011
    // audit: COBIL00C-012
    #[test]
    fn the_account_is_checked_before_the_confirmation() -> TestResult {
        for (raw, parsed) in [
            ("Y", Confirm::Yes),
            ("y", Confirm::Yes),
            ("N", Confirm::No),
            ("n", Confirm::No),
            ("", Confirm::Blank),
            ("X", Confirm::Invalid),
        ] {
            assert_eq!(confirm(raw), parsed, "{raw:?}");
        }
        let empty = edit(&BillPayInput {
            acct_id: "",
            confirm: "N",
        })
        .err()
        .ok_or("a blank account passed")?;
        assert_eq!(
            (empty.message(), empty.field(), empty.class().http_status()),
            ("Acct ID can NOT be empty...", BillPayField::AcctId, 400)
        );
        let id = PicU::new(1)?;
        assert_eq!(edit_account("00000000001")?, AccountKey::Id(id));
        assert_eq!(edit_account("1")?, AccountKey::Unmatched);
        assert_eq!(
            edit(&BillPayInput {
                acct_id: "00000000001",
                confirm: "N"
            })?,
            EnterAction::Clear
        );
        assert_eq!(
            edit(&BillPayInput {
                acct_id: "00000000001",
                confirm: "y"
            })?,
            EnterAction::ReadAccount {
                id,
                intent: ReadIntent::Pay
            }
        );
        assert_eq!(
            edit(&BillPayInput {
                acct_id: "00000000001",
                confirm: ""
            })?,
            EnterAction::ReadAccount {
                id,
                intent: ReadIntent::ShowBalance
            }
        );
        let invalid = edit(&BillPayInput {
            acct_id: "00000000001",
            confirm: "X",
        })
        .err()
        .ok_or("X passed")?;
        assert_eq!(
            (invalid.message(), invalid.field()),
            (
                "Invalid value. Valid values are (Y/N)...",
                BillPayField::Confirm
            )
        );
        Ok(())
    }

    // audit: COBIL00C-014
    // audit: COBIL00C-015
    // audit: COBIL00C-017
    // audit: COBIL00C-018
    // audit: COBIL00C-019
    #[test]
    fn a_positive_balance_is_shown_or_paid_and_none_cannot_be_paid() -> TestResult {
        let first = account(1)?;
        let shown = decide(ReadIntent::ShowBalance, &first)?;
        let BillPayDecision::ShowBalance(outcome) = shown else {
            return Err("COBIL00C-019: a blank confirmation paid".into());
        };
        assert_eq!(outcome.balance(), Some("+0000000194.00"));
        assert_eq!(
            outcome.balance(),
            Some(balance_display(first.curr_bal).as_str())
        );
        assert_eq!(
            outcome.message().as_deref(),
            Some("Confirm to make a bill payment...")
        );
        assert_eq!(outcome.acct_id(), Some(first.id));
        assert_eq!(decide(ReadIntent::Pay, &first)?, BillPayDecision::Pay);
        let mut empty = first.clone();
        empty.curr_bal = domain::Money::ZERO;
        let nothing = decide(ReadIntent::Pay, &empty)
            .err()
            .ok_or("a zero balance was paid")?;
        assert_eq!(
            (
                nothing.message(),
                nothing.field(),
                nothing.class().http_status()
            ),
            ("You have nothing to pay...", BillPayField::AcctId, 400)
        );
        assert_eq!(account_not_found().message(), "Account ID NOT found...");
        assert_eq!(account_not_found().class().http_status(), 404);
        assert_eq!(
            account_lookup_failed().message(),
            "Unable to lookup Account..."
        );
        assert_eq!(account_lookup_failed().class().http_status(), 500);
        Ok(())
    }

    // audit: COBIL00C-020
    // audit: COBIL00C-025
    // audit: COBIL00C-027
    #[test]
    fn the_payment_transaction_carries_the_fixed_fields_and_one_timestamp() -> TestResult {
        let first = account(1)?;
        let xref = fixture::<CardXrefRecord>("cardxref.txt")?
            .into_iter()
            .find(|record| record.acct_id == first.id)
            .ok_or("no cross-reference for account 1")?;
        let new_id = PicX::from_text("0000000996722788")?;
        let effects = payment(new_id.clone(), None, &first, Some(&xref), &clock()?);
        let record = &effects.transaction;
        assert_eq!(record.id, new_id);
        assert!(record.card_num == xref.card_num, "COBIL00C-020");
        assert_eq!(record.type_cd.as_bytes(), b"02");
        assert_eq!(record.cat_cd.get(), 2);
        assert_eq!(record.source.to_text(), "POS TERM");
        assert_eq!(record.desc.to_text(), "BILL PAYMENT - ONLINE");
        assert_eq!(record.merchant_id.get(), 999_999_999);
        assert_eq!(record.merchant_name.to_text(), "BILL PAYMENT");
        assert_eq!(record.merchant_city.to_text(), "N/A");
        assert_eq!(record.merchant_zip.to_text(), "N/A");
        assert_eq!(record.orig_ts, record.proc_ts);
        let stamp = record
            .orig_ts
            .value()
            .ok_or("COBIL00C-027: a blank timestamp")?;
        assert_eq!(stamp.form(), TsForm::Iso);
        let mut text = Vec::new();
        record.orig_ts.encode_into(&mut text);
        assert!(
            has_layout(&text, b"9999-99-99 99:99:99.000000"),
            "COBIL00C-027"
        );
        assert_eq!(effects.account.id, first.id);
        Ok(())
    }

    // audit: COBIL00C-028
    // audit: COBIL00C-029
    // audit: COBIL00C-030
    // audit: COBIL00C-032
    // audit: COBIL00C-033
    #[test]
    fn the_last_failing_step_sets_the_payment_message() -> TestResult {
        let new_id = PicX::from_text("0000000996722788")?;
        let message = success_message(&new_id);
        assert_eq!(
            message,
            "Payment successful.  Your Transaction ID is 0000000996722788."
        );
        assert_eq!(message.len(), 61);
        let paid = payment_outcome(&new_id, TranWrite::Written, AccountRewrite::Rewritten)?;
        assert_eq!(
            paid,
            BillPayOutcome::Paid {
                tran_id: new_id.clone()
            }
        );
        assert_eq!(paid.message(), Some(message));
        assert_eq!(paid.tran_id(), Some(&new_id));
        for (write, rewrite, text, status) in [
            (
                TranWrite::Duplicate,
                AccountRewrite::Rewritten,
                "Tran ID already exist...",
                500,
            ),
            (
                TranWrite::Failed,
                AccountRewrite::Rewritten,
                "Unable to Add Bill pay Transaction...",
                500,
            ),
            (
                TranWrite::Written,
                AccountRewrite::NotFound,
                "Account ID NOT found...",
                404,
            ),
            (
                TranWrite::Written,
                AccountRewrite::Failed,
                "Unable to Update Account...",
                500,
            ),
        ] {
            let failure = payment_outcome(&new_id, write, rewrite)
                .err()
                .ok_or("a failed step paid")?;
            assert_eq!(
                (
                    failure.message(),
                    failure.field(),
                    failure.class().http_status()
                ),
                (text, BillPayField::AcctId, status)
            );
        }
        assert_eq!(BillPayOutcome::Cleared.message(), None);
        Ok(())
    }
}

mod cousr00c {
    use domain::batch::loads::decode_usrsec;
    use domain::copybook::PicX;
    use domain::online::cousr00c::{
        Action, DEFAULT_PAGE_SIZE, Direction, LOW_KEY, ListMessage, NextPage, PageRows,
        PagingState, Plan, UserListFailure, UserListRow, UserPage, page, plan, start_user_id,
    };

    use super::TestResult;

    type Outcome<T> = Result<T, Box<dyn std::error::Error>>;

    /// The ten in-stream users of `DUSRSECJ.jcl`, in key order, as list rows.
    fn fixture_users() -> Outcome<Vec<UserListRow>> {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../app/jcl/DUSRSECJ.jcl");
        let jcl = std::fs::read(path)?;
        Ok(decode_usrsec(&jcl)?
            .iter()
            .map(|framed| UserListRow::from(&framed.record))
            .collect())
    }

    /// `count` users `U0000001`, `U0000002`, …, in key order.
    fn numbered(count: u32) -> Vec<UserListRow> {
        (1..=count)
            .map(|n| UserListRow {
                user_id: PicX::moved_from(format!("U{n:07}").as_bytes()),
                first_name: PicX::moved_from(b"FIRST"),
                last_name: PicX::moved_from(b"LAST"),
                user_type: PicX::moved_from(b"U"),
            })
            .collect()
    }

    /// One interaction against `users`, read as the page queries read the table.
    fn interact(users: &[UserListRow], action: Action, state: PagingState) -> Outcome<UserPage> {
        Ok(match plan(action, state, DEFAULT_PAGE_SIZE)? {
            Plan::Respond(answer) => answer,
            Plan::Read(read) => {
                let at = read.key().clone();
                let matching: Vec<UserListRow> = match read.direction() {
                    Direction::Forward => users
                        .iter()
                        .filter(|user| user.user_id >= at)
                        .cloned()
                        .collect(),
                    Direction::Backward => users
                        .iter()
                        .rev()
                        .filter(|user| user.user_id <= at)
                        .cloned()
                        .collect(),
                };
                let limit = read.fetch_limit();
                page(read, matching.into_iter().take(limit).collect())?
            }
        })
    }

    /// The user IDs of the ten row positions, blank for an empty position.
    fn shown(answer: &UserPage) -> Outcome<Vec<String>> {
        match &answer.rows {
            PageRows::Replace(rows) => Ok(rows
                .iter()
                .map(|row| {
                    row.as_ref()
                        .map(|user| user.user_id.to_text())
                        .unwrap_or_default()
                })
                .collect()),
            PageRows::Keep => Err("the rows were kept".into()),
        }
    }

    fn enter(users: &[UserListRow], search: &str) -> Outcome<UserPage> {
        interact(
            users,
            Action::Enter {
                start: start_user_id(search),
            },
            PagingState::default(),
        )
    }

    // audit: COUSR00C-012
    // audit: COUSR00C-013
    // audit: COUSR00C-014
    // audit: COUSR00C-015
    // audit: COUSR00C-016
    // audit: COUSR00C-028
    #[test]
    fn enter_lists_from_the_search_key_in_user_id_order() -> TestResult {
        let users = fixture_users()?;
        assert_eq!(DEFAULT_PAGE_SIZE, 10);
        assert_eq!(start_user_id(""), LOW_KEY);
        assert_eq!(start_user_id("user"), PicX::moved_from(b"user    "));
        let all = enter(&users, "")?;
        let expected = [
            "ADMIN001", "ADMIN002", "ADMIN003", "ADMIN004", "ADMIN005", "USER0001", "USER0002",
            "USER0003", "USER0004", "USER0005",
        ];
        assert_eq!(shown(&all)?, expected);
        assert_eq!(
            (all.state.page_no.get(), all.state.next_page),
            (1, NextPage::No)
        );
        assert_eq!(
            (all.state.first_key.to_text(), all.state.last_key.to_text()),
            ("ADMIN001".to_owned(), "USER0005".to_owned())
        );
        assert_eq!(
            all.message_text(),
            Some("You have reached the bottom of the page...")
        );
        let PageRows::Replace(rows) = &all.rows else {
            return Err("COUSR00C-028: rows kept".into());
        };
        let first_user = rows
            .get(5)
            .cloned()
            .flatten()
            .ok_or("COUSR00C-028: row 6 is blank")?;
        assert_eq!(first_user.first_name.to_text(), "LAWRENCE");
        assert_eq!(
            (
                first_user.last_name.to_text(),
                first_user.user_type.to_text()
            ),
            ("THOMAS".to_owned(), "U".to_owned())
        );
        let tail = enter(&users, "USER0003")?;
        assert_eq!(shown(&tail)?.iter().filter(|id| !id.is_empty()).count(), 3);
        assert_eq!(
            (tail.state.page_no.get(), tail.state.next_page),
            (1, NextPage::No)
        );
        assert_eq!(tail.state.first_key.to_text(), "USER0003");
        assert!(tail.state.last_key.is_spaces(), "COUSR00C-016");
        let more = enter(&numbered(25), "")?;
        assert_eq!((more.state.next_page, more.message), (NextPage::Yes, None));
        assert_eq!(more.state.last_key.to_text(), "U0000010");
        Ok(())
    }

    // audit: COUSR00C-017
    // audit: COUSR00C-022
    // audit: COUSR00C-023
    #[test]
    fn f8_and_f7_page_from_the_keys_the_state_carries() -> TestResult {
        let users = numbered(25);
        let first = enter(&users, "")?;
        let second = interact(&users, Action::Page(Direction::Forward), first.state)?;
        assert_eq!(
            shown(&second)?.first().map(String::as_str),
            Some("U0000011")
        );
        assert_eq!(
            (second.state.page_no.get(), second.state.next_page),
            (2, NextPage::Yes)
        );
        let third = interact(&users, Action::Page(Direction::Forward), second.state)?;
        assert_eq!(shown(&third)?.iter().filter(|id| !id.is_empty()).count(), 5);
        assert_eq!(
            (third.state.page_no.get(), third.message_text()),
            (3, Some("You have reached the bottom of the page..."))
        );
        let back = interact(&users, Action::Page(Direction::Backward), third.state)?;
        assert_eq!(shown(&back)?.first().map(String::as_str), Some("U0000011"));
        assert_eq!(shown(&back)?.last().map(String::as_str), Some("U0000020"));
        assert_eq!(
            (back.state.page_no.get(), back.state.next_page, back.message),
            (2, NextPage::Yes, None)
        );
        let top = interact(&users, Action::Page(Direction::Backward), back.state)?;
        assert_eq!(shown(&top)?.first().map(String::as_str), Some("U0000001"));
        assert_eq!(
            (top.state.page_no.get(), top.message_text()),
            (1, Some("You have reached the top of the page..."))
        );
        Ok(())
    }

    // audit: COUSR00C-018
    // audit: COUSR00C-019
    // audit: COUSR00C-020
    // audit: COUSR00C-021
    // audit: COUSR00C-024
    #[test]
    fn the_edges_of_the_list_answer_with_their_messages() -> TestResult {
        let users = fixture_users()?;
        let first = enter(&users, "")?;
        let up = interact(
            &users,
            Action::Page(Direction::Backward),
            first.state.clone(),
        )?;
        assert_eq!(
            (up.rows.clone(), up.message),
            (PageRows::Keep, Some(ListMessage::AlreadyAtTop))
        );
        assert_eq!(
            up.message_text(),
            Some("You are already at the top of the page...")
        );
        assert_eq!(
            up.state,
            PagingState {
                next_page: NextPage::Yes,
                ..first.state.clone()
            }
        );
        let down = interact(&users, Action::Page(Direction::Forward), up.state.clone())?;
        assert_eq!(shown(&down)?, vec![String::new(); 10]);
        assert_eq!(
            (down.state.page_no.get(), down.state.next_page),
            (1, NextPage::No)
        );
        assert_eq!(
            down.message_text(),
            Some("You have reached the bottom of the page...")
        );
        assert_eq!(
            (down.state.first_key.clone(), down.state.last_key.clone()),
            (up.state.first_key, up.state.last_key)
        );
        let stuck = interact(
            &users,
            Action::Page(Direction::Forward),
            first.state.clone(),
        )?;
        assert_eq!(
            stuck.message_text(),
            Some("You are already at the bottom of the page...")
        );
        assert_eq!((stuck.rows, stuck.state), (PageRows::Keep, first.state));
        let numbered = numbered(25);
        let late = PagingState {
            first_key: PicX::moved_from(b"U0000005"),
            last_key: PicX::moved_from(b"U0000014"),
            page_no: domain::copybook::PicU::new(3)?,
            next_page: NextPage::Yes,
        };
        let short = interact(&numbered, Action::Page(Direction::Backward), late.clone())?;
        let rows = shown(&short)?;
        assert_eq!(rows.iter().filter(|id| !id.is_empty()).count(), 4);
        assert_eq!(
            (
                rows.get(6).map(String::as_str),
                rows.last().map(String::as_str)
            ),
            (Some("U0000001"), Some("U0000004"))
        );
        assert_eq!(
            (short.state.page_no.get(), short.state.first_key.clone()),
            (3, late.first_key)
        );
        assert_eq!(
            short.message_text(),
            Some("You have reached the top of the page...")
        );
        Ok(())
    }

    // audit: COUSR00C-026
    #[test]
    fn a_failing_read_is_a_lookup_error() -> TestResult {
        let users = fixture_users()?;
        let Plan::Read(read) = plan(
            Action::Enter { start: LOW_KEY },
            PagingState::default(),
            DEFAULT_PAGE_SIZE,
        )?
        else {
            return Err("ENTER answered without a read".into());
        };
        assert_eq!(read.fetch_limit(), 11);
        let disordered: Vec<UserListRow> = users.iter().rev().cloned().collect();
        let failure = page(read, disordered)
            .err()
            .ok_or("rows out of key order were paged")?;
        assert_eq!(failure, UserListFailure::LookupFailed);
        assert_eq!(
            (failure.message(), failure.class().http_status()),
            ("Unable to lookup User...", 500)
        );
        Ok(())
    }

    /// HALTED(X-COUSR00C-H1, X-COUSR00C-H2): a start key past every user and F8 with a blank last
    /// key run here for coverage only; no value is asserted.
    #[test]
    fn halted_browse_starts_run_without_parity_assertions() -> TestResult {
        let users = fixture_users()?;
        let _ = enter(&users, "ZZZZZZZZ")?;
        let blank_last = PagingState {
            next_page: NextPage::Yes,
            ..PagingState::default()
        };
        let _ = interact(&users, Action::Page(Direction::Forward), blank_last);
        Ok(())
    }
}

mod cousr01c {
    use domain::copybook::PicX;
    use domain::online::cousr01c::{
        UserAddField, UserAddInput, added_message, duplicate_user_id, edit, write_failure,
    };

    use super::TestResult;

    const FULL: UserAddInput<'static> = UserAddInput {
        first_name: " Ann",
        last_name: "lee",
        user_id: "newusr01",
        password: "secret",
        user_type: "x",
    };

    // audit: COUSR01C-010
    // audit: COUSR01C-011
    // audit: COUSR01C-012
    // audit: COUSR01C-013
    // audit: COUSR01C-014
    #[test]
    fn the_five_fields_are_required_in_screen_order() -> TestResult {
        let cases = [
            (
                UserAddInput {
                    first_name: "",
                    last_name: "",
                    ..FULL
                },
                "First Name can NOT be empty...",
                UserAddField::FirstName,
            ),
            (
                UserAddInput {
                    last_name: "  ",
                    user_id: "",
                    ..FULL
                },
                "Last Name can NOT be empty...",
                UserAddField::LastName,
            ),
            (
                UserAddInput {
                    user_id: "",
                    password: "",
                    ..FULL
                },
                "User ID can NOT be empty...",
                UserAddField::UserId,
            ),
            (
                UserAddInput {
                    password: "",
                    user_type: "",
                    ..FULL
                },
                "Password can NOT be empty...",
                UserAddField::Password,
            ),
            (
                UserAddInput {
                    user_type: " ",
                    ..FULL
                },
                "User Type can NOT be empty...",
                UserAddField::UserType,
            ),
        ];
        for (input, message, field) in cases {
            let failure = edit(&input).err().ok_or("an empty field passed")?;
            assert_eq!(
                (
                    failure.message(),
                    failure.field(),
                    failure.class().http_status()
                ),
                (message, field, 400)
            );
        }
        assert_eq!(UserAddField::FirstName.name(), "first_name");
        assert_eq!(UserAddField::UserType.name(), "user_type");
        Ok(())
    }

    // audit: COUSR01C-015
    // audit: COUSR01C-016
    #[test]
    fn any_type_is_accepted_and_every_value_is_kept_as_typed() -> TestResult {
        let user = edit(&FULL)?;
        assert_eq!(user.first_name(), &PicX::moved_from(b" Ann"));
        assert_eq!(user.last_name(), &PicX::moved_from(b"lee"));
        assert_eq!(user.user_id(), &PicX::moved_from(b"newusr01"));
        assert_eq!(user.user_type(), &PicX::moved_from(b"x"));
        assert!(user.password().expose() == b"secret  ", "COUSR01C-016");
        for user_type in ["A", "U", "a", "9"] {
            assert_eq!(
                edit(&UserAddInput { user_type, ..FULL })?
                    .user_type()
                    .to_text(),
                user_type
            );
        }
        let record = user.into_record();
        assert_eq!(
            (record.id.to_text(), record.fname.as_bytes().first()),
            ("newusr01".to_owned(), Some(&b' '))
        );
        assert!(record.pwd.expose() == b"secret  ", "COUSR01C-016");
        Ok(())
    }

    // audit: COUSR01C-021
    // audit: COUSR01C-023
    // audit: COUSR01C-024
    #[test]
    fn the_insert_outcomes_carry_their_messages() {
        assert_eq!(
            added_message(&PicX::moved_from(b"NEWUSR01")),
            "User NEWUSR01 has been added ..."
        );
        assert_eq!(
            added_message(&PicX::moved_from(b"AB CD")),
            "User AB has been added ..."
        );
        let duplicate = duplicate_user_id();
        assert_eq!(
            (
                duplicate.message(),
                duplicate.field(),
                duplicate.class().http_status()
            ),
            ("User ID already exist...", UserAddField::UserId, 400)
        );
        let failed = write_failure();
        assert_eq!(
            (
                failed.message(),
                failed.field(),
                failed.class().http_status()
            ),
            ("Unable to Add User...", UserAddField::FirstName, 500)
        );
    }
}

mod cousr02c {
    use domain::copybook::PicX;
    use domain::online::cousr02c::{
        RewriteResult, StoredUserView, UserField, UserMaintFailure, UserRead, UserUpdateInput,
        edit_update, edit_user_id, modified, read, reread, rewrite_outcome, updated_message,
    };

    use super::TestResult;

    fn stored() -> StoredUserView {
        StoredUserView {
            user_id: PicX::moved_from(b"USER0001"),
            first_name: PicX::moved_from(b"LAWRENCE"),
            last_name: PicX::moved_from(b"THOMAS"),
            user_type: PicX::moved_from(b"U"),
        }
    }

    const SAME: UserUpdateInput<'static> = UserUpdateInput {
        first_name: "LAWRENCE",
        last_name: "THOMAS",
        password: None,
        user_type: "U",
    };

    fn failure_of<T>(
        result: Result<T, UserMaintFailure>,
    ) -> Result<(&'static str, UserField, u16), Box<dyn std::error::Error>> {
        let failure = result.err().ok_or("the step passed")?;
        Ok((
            failure.message(),
            failure.field(),
            failure.class().http_status(),
        ))
    }

    // audit: COUSR02C-010
    // audit: COUSR02C-011
    // audit: COUSR02C-012
    // audit: COUSR02C-015
    // audit: COUSR02C-016
    #[test]
    fn the_fetch_keys_by_the_id_as_received_and_shows_no_password() -> TestResult {
        assert_eq!(
            failure_of(edit_user_id("   "))?,
            ("User ID can NOT be empty...", UserField::UserId, 400)
        );
        assert_eq!(edit_user_id("admin001")?, PicX::moved_from(b"admin001"));
        assert_eq!(edit_user_id(" USER1")?, PicX::moved_from(b" USER1"));
        let row = stored();
        let view = read(UserRead::Found(&row))?;
        assert_eq!(
            (view.user_id.to_text(), view.first_name.to_text()),
            ("USER0001".to_owned(), "LAWRENCE".to_owned())
        );
        assert_eq!(
            (view.last_name.to_text(), view.user_type.to_text()),
            ("THOMAS".to_owned(), "U".to_owned())
        );
        assert_eq!(
            failure_of(read(UserRead::NotFound))?,
            ("User ID NOT found...", UserField::UserId, 404)
        );
        assert_eq!(
            failure_of(read(UserRead::Error))?,
            ("Unable to lookup User...", UserField::FirstName, 500)
        );
        assert_eq!(reread(UserRead::Found(&row))?, &row);
        Ok(())
    }

    // audit: COUSR02C-018
    // audit: COUSR02C-019
    // audit: COUSR02C-020
    // audit: COUSR02C-021
    // audit: COUSR02C-022
    // audit: COUSR02C-023
    #[test]
    fn the_update_edits_run_in_screen_order() -> TestResult {
        let blank_id = edit_update(
            " ",
            &UserUpdateInput {
                first_name: "",
                ..SAME
            },
        );
        assert_eq!(
            failure_of(blank_id)?,
            ("User ID can NOT be empty...", UserField::UserId, 400)
        );
        let cases = [
            (
                UserUpdateInput {
                    first_name: "",
                    last_name: "",
                    ..SAME
                },
                "First Name can NOT be empty...",
                UserField::FirstName,
            ),
            (
                UserUpdateInput {
                    last_name: " ",
                    password: Some(""),
                    ..SAME
                },
                "Last Name can NOT be empty...",
                UserField::LastName,
            ),
            (
                UserUpdateInput {
                    password: Some("  "),
                    user_type: "",
                    ..SAME
                },
                "Password can NOT be empty...",
                UserField::Password,
            ),
            (
                UserUpdateInput {
                    password: Some("NEWPASS"),
                    user_type: "",
                    ..SAME
                },
                "User Type can NOT be empty...",
                UserField::UserType,
            ),
        ];
        for (input, message, field) in cases {
            assert_eq!(
                failure_of(edit_update("USER0001", &input))?,
                (message, field, 400)
            );
        }
        let free = edit_update(
            "USER0001",
            &UserUpdateInput {
                first_name: "law2",
                user_type: "a",
                ..SAME
            },
        )?;
        assert_eq!(
            (free.first_name().to_text(), free.user_type().to_text()),
            ("law2".to_owned(), "a".to_owned())
        );
        assert!(free.password().is_none(), "COUSR02C-021");
        Ok(())
    }

    // audit: COUSR02C-026
    // audit: COUSR02C-027
    // audit: COUSR02C-028
    #[test]
    fn only_a_changed_value_is_a_modification() -> TestResult {
        let row = stored();
        // An omitted password is unchanged (E27), so no hash is consulted and no argon2 work runs.
        let unchanged = edit_update("USER0001", &SAME)?;
        let failure = modified(&unchanged, &row, "not-consulted")
            .err()
            .ok_or("an unchanged update passed")?;
        assert_eq!(
            (
                failure.message(),
                failure.field(),
                failure.class().http_status()
            ),
            ("Please modify to update ...", UserField::FirstName, 400)
        );
        let case_only = edit_update(
            "USER0001",
            &UserUpdateInput {
                last_name: "Thomas",
                ..SAME
            },
        )?;
        let changes = modified(&case_only, &row, "not-consulted")?;
        assert_eq!(changes.last_name().to_text(), "Thomas");
        assert_eq!(
            (changes.user_id(), changes.first_name()),
            (&row.user_id, &row.first_name)
        );
        assert!(changes.new_password().is_none(), "COUSR02C-027");
        let retyped = edit_update(
            "USER0001",
            &UserUpdateInput {
                user_type: "A",
                ..SAME
            },
        )?;
        assert_eq!(
            modified(&retyped, &row, "not-consulted")?
                .user_type()
                .to_text(),
            "A"
        );
        Ok(())
    }

    // audit: COUSR02C-029
    // audit: COUSR02C-030
    // audit: COUSR02C-031
    // audit: COUSR02C-032
    #[test]
    fn the_rewrite_outcomes_carry_their_messages() -> TestResult {
        let id = PicX::moved_from(b"USER0001");
        assert_eq!(
            rewrite_outcome(&id, RewriteResult::Rewritten)?,
            "User USER0001 has been updated ..."
        );
        assert_eq!(
            updated_message(&PicX::moved_from(b"AB CD")),
            "User AB has been updated ..."
        );
        assert_eq!(
            updated_message(&PicX::moved_from(b" AB")),
            "User  has been updated ..."
        );
        assert_eq!(
            failure_of(rewrite_outcome(&id, RewriteResult::NotFound))?,
            ("User ID NOT found...", UserField::UserId, 404)
        );
        assert_eq!(
            failure_of(rewrite_outcome(&id, RewriteResult::Error))?,
            ("Unable to Update User...", UserField::FirstName, 500)
        );
        assert_eq!(
            UserMaintFailure::update_failed().message(),
            "Unable to Update User..."
        );
        assert_eq!(
            UserMaintFailure::not_found().message(),
            "User ID NOT found..."
        );
        Ok(())
    }
}

mod cousr03c {
    use domain::copybook::PicX;
    use domain::online::cousr02c::{StoredUserView, UserField, UserMaintFailure, UserRead};
    use domain::online::cousr03c::{
        DeleteResult, delete_outcome, deleted_message, edit_delete, held_for_delete,
    };

    use super::TestResult;

    fn failure_of<T>(
        result: Result<T, UserMaintFailure>,
    ) -> Result<(&'static str, UserField, u16), Box<dyn std::error::Error>> {
        let failure = result.err().ok_or("the step passed")?;
        Ok((
            failure.message(),
            failure.field(),
            failure.class().http_status(),
        ))
    }

    // audit: COUSR03C-010
    // audit: COUSR03C-016
    // audit: COUSR03C-017
    #[test]
    fn the_delete_key_is_required_and_kept_as_received() -> TestResult {
        assert_eq!(
            failure_of(edit_delete(""))?,
            ("User ID can NOT be empty...", UserField::UserId, 400)
        );
        assert_eq!(edit_delete("user0001")?, PicX::moved_from(b"user0001"));
        let row = StoredUserView {
            user_id: PicX::moved_from(b"USER0001"),
            first_name: PicX::moved_from(b"LAWRENCE"),
            last_name: PicX::moved_from(b"THOMAS"),
            user_type: PicX::moved_from(b"U"),
        };
        assert_eq!(held_for_delete(UserRead::Found(&row))?, &row.user_id);
        // HALTED(X-COUSR03C-H1): a failed read before the delete has no COBOL result; run only.
        let _ = held_for_delete(UserRead::NotFound);
        Ok(())
    }

    // audit: COUSR03C-019
    // audit: COUSR03C-020
    // audit: COUSR03C-021
    // audit: COUSR03C-022
    // audit: COUSR03C-024
    #[test]
    fn the_delete_outcomes_carry_their_messages_and_the_update_defect() -> TestResult {
        let admin = PicX::moved_from(b"ADMIN001");
        assert_eq!(
            delete_outcome(&admin, DeleteResult::Deleted)?,
            "User ADMIN001 has been deleted ..."
        );
        assert_eq!(
            deleted_message(&PicX::moved_from(b"AB CD")),
            "User AB has been deleted ..."
        );
        assert_eq!(
            deleted_message(&PicX::moved_from(b" AB")),
            "User  has been deleted ..."
        );
        assert_eq!(
            failure_of(delete_outcome(&admin, DeleteResult::NotFound))?,
            ("User ID NOT found...", UserField::UserId, 404)
        );
        assert_eq!(
            failure_of(delete_outcome(&admin, DeleteResult::Error))?,
            ("Unable to Update User...", UserField::FirstName, 500)
        );
        Ok(())
    }
}

/// The authorization detail the COPAUS* modules share: no ASCII fixture holds authorizations, so
/// one is built on the keys of the first cross-reference record (account 50, customer 50).
mod authorization {
    use domain::copybook::{PicS, PicU, PicX};
    use domain::money::Decimal;
    use domain::online::copaus0c::AuthKey;
    use domain::records::PendingAuthDetailRecord;
    use domain::{CardNumber, Money};

    /// The card number of `cardxref.txt` line 1.
    pub const CARD: &str = "0500024453765740";

    /// The key CP00 writes for 2022-01-15 10:30:45.123: `99999 − 22015` and
    /// `999999999 − 103045123`.
    pub fn key_2022_01_15() -> Result<AuthKey, Box<dyn std::error::Error>> {
        Ok(AuthKey::new(PicS::new(77_984)?, PicS::new(896_954_876)?))
    }

    /// A detail with this key, stored `YYMMDD` date and `HHMMSS` time, response code and reason,
    /// and approved amount in cents.
    pub fn detail(
        key: AuthKey,
        orig: (&str, &str),
        response: (&str, &str),
        approved_cents: i64,
    ) -> Result<PendingAuthDetailRecord, Box<dyn std::error::Error>> {
        Ok(PendingAuthDetailRecord {
            auth_date_9c: key.date_9c(),
            auth_time_9c: key.time_9c(),
            orig_date: PicX::from_text(orig.0)?,
            orig_time: PicX::from_text(orig.1)?,
            card_num: CardNumber::parse(CARD)?,
            auth_type: PicX::from_text("SALE")?,
            card_expiry_date: PicX::from_text("0927")?,
            message_type: PicX::from_text("0100")?,
            message_source: PicX::from_text("POS")?,
            auth_id_code: PicX::from_text("000001")?,
            auth_resp_code: PicX::from_text(response.0)?,
            auth_resp_reason: PicX::from_text(response.1)?,
            processing_code: PicU::new(1_234)?,
            transaction_amt: Money::new(Decimal::new(2_500, 2))?,
            approved_amt: Money::new(Decimal::new(approved_cents, 2))?,
            merchant_catagory_code: PicX::from_text("5411")?,
            acqr_country_code: PicX::from_text("840")?,
            pos_entry_mode: PicU::new(5)?,
            merchant_id: PicX::from_text("MERCHANT0000001")?,
            merchant_name: PicX::from_text("CORNER GROCERY")?,
            merchant_city: PicX::from_text("SEATTLE")?,
            merchant_state: PicX::from_text("WA")?,
            merchant_zip: PicX::from_text("98101")?,
            transaction_id: PicX::from_text("TXN000000000001")?,
            match_status: PicX::from_text("P")?,
            auth_fraud: PicX::spaces(),
            fraud_rpt_date: PicX::spaces(),
            filler: PicX::spaces(),
        })
    }
}

mod copaus0c {
    use domain::Money;
    use domain::copybook::{Count, PicS, PicU, PicX};
    use domain::money::Decimal;
    use domain::online::copaus0c::{
        AuthKey, AuthSummaryFailure, AuthSummaryField, DEFAULT_PAGE_SIZE, Direction, Lookup,
        MessageCodes, NextPage, PageRequest, SummaryBlock, SummaryState, XrefKeys, detail_failure,
        edit_account, page, row_from, summary_block, summary_key, xref_keys,
    };
    use domain::records::{
        AccountRecord, CardXrefRecord, CustomerRecord, PendingAuthSummaryRecord,
    };

    use super::authorization::{CARD, detail, key_2022_01_15};
    use super::{TestResult, fixture};

    type Outcome<T> = Result<T, Box<dyn std::error::Error>>;
    type Shown = (String, Option<AuthSummaryField>, u16, MessageCodes);

    fn shown<T>(result: Result<T, AuthSummaryFailure>) -> Outcome<Shown> {
        let failure = result.err().ok_or("the step passed")?;
        Ok((
            failure.message().to_owned(),
            failure.field(),
            failure.class().http_status(),
            failure.codes(),
        ))
    }

    /// The keys of `cardxref.txt` line 1: account 50, customer 50.
    fn keys() -> Outcome<XrefKeys> {
        let xref = fixture::<CardXrefRecord>("cardxref.txt")?
            .into_iter()
            .next()
            .ok_or("no cross-reference")?;
        Ok(xref_keys(xref.acct_id, Lookup::Found(&xref))?)
    }

    /// Account 50 with these credit and cash limits in cents.
    fn account(credit_cents: i64, cash_cents: i64) -> Outcome<AccountRecord> {
        let mut account = fixture::<AccountRecord>("acctdata.txt")?
            .into_iter()
            .find(|record| record.id.get() == 50)
            .ok_or("no account 50")?;
        account.credit_limit = Money::new(Decimal::new(credit_cents, 2))?;
        account.cash_credit_limit = Money::new(Decimal::new(cash_cents, 2))?;
        Ok(account)
    }

    /// Customer 50 renamed and readdressed with invented values.
    fn customer() -> Outcome<CustomerRecord> {
        let mut customer = fixture::<CustomerRecord>("custdata.txt")?
            .into_iter()
            .find(|record| record.id.get() == 50)
            .ok_or("no customer 50")?;
        customer.first_name = PicX::from_text("ANN MARIE")?;
        customer.middle_name = PicX::from_text("LOU")?;
        customer.last_name = PicX::from_text("DOE")?;
        customer.addr_line_1 = PicX::from_text("12 MAIN STREET  APT 4")?;
        customer.addr_line_2 = PicX::from_text("BLDG 7")?;
        customer.addr_line_3 = PicX::from_text("SPRINGFIELD")?;
        customer.addr_state_cd = PicX::from_text("IL")?;
        customer.addr_zip = PicX::from_text("62701-1234")?;
        customer.phone_num_1 = PicX::from_text("(217)555-0100XY")?;
        Ok(customer)
    }

    fn block_for(summary: Option<&PendingAuthSummaryRecord>) -> Outcome<SummaryBlock> {
        let found = summary.map_or(Lookup::NotFound, Lookup::Found);
        Ok(summary_block(
            keys()?,
            Lookup::Found(&account(123_456, 1_234_567_890)?),
            Lookup::Found(&customer()?),
            found,
        )?)
    }

    /// `count` approved details of account 50, keys ascending from (77800, 896999999).
    fn details(count: i64) -> Outcome<Vec<domain::records::PendingAuthDetailRecord>> {
        (0..count)
            .map(|offset| {
                detail(
                    AuthKey::new(PicS::new(77_800)?, PicS::new(896_999_999 - offset)?),
                    ("220718", "103000"),
                    ("00", "0000"),
                    2_500,
                )
            })
            .rev()
            .collect()
    }

    // audit: COPAUS0C-005
    // audit: COPAUS0C-006
    // audit: COPAUS0C-008
    #[test]
    fn the_account_must_be_eleven_digits() -> TestResult {
        for raw in ["", "           ", "\0\0\0"] {
            assert_eq!(
                shown(edit_account(raw))?,
                (
                    "Please enter Acct Id...".to_owned(),
                    Some(AuthSummaryField::AcctId),
                    400,
                    MessageCodes::None
                )
            );
        }
        for raw in [
            "50",
            " 0000000050",
            "0000000005O",
            "000000000500",
            "+0000000050",
        ] {
            assert_eq!(
                shown(edit_account(raw))?,
                (
                    "Acct Id must be Numeric ...".to_owned(),
                    Some(AuthSummaryField::AcctId),
                    400,
                    MessageCodes::None
                )
            );
        }
        assert_eq!(edit_account("00000000050")?, PicU::new(50)?);
        Ok(())
    }

    // audit: COPAUS0C-013
    // audit: COPAUS0C-014
    // audit: COPAUS0C-015
    #[test]
    fn the_cross_reference_keys_the_reads_or_ends_the_request() -> TestResult {
        let keys = keys()?;
        assert_eq!(
            (keys.acct_id(), keys.cust_id()),
            (PicU::new(50)?, PicU::new(50)?)
        );
        let acct = PicU::new(50)?;
        let (missing, field, status, codes) = shown(xref_keys(acct, Lookup::NotFound))?;
        assert!(
            missing.starts_with("Account:00000000050 not found in XREF file. Resp:"),
            "COPAUS0C-014"
        );
        assert_eq!(
            (missing.chars().count(), missing.get(58..64)),
            (73, Some(" Reas:"))
        );
        assert_eq!(
            (field, status, codes),
            (Some(AuthSummaryField::AcctId), 404, MessageCodes::HaltedH4)
        );
        let (failed, field, status, codes) = shown(xref_keys(acct, Lookup::Failed))?;
        assert!(
            failed.starts_with("Account:00000000050 System error while reading XREF file. Resp:"),
            "COPAUS0C-015"
        );
        assert!(failed.ends_with(" Reas:"), "COPAUS0C-015");
        assert_eq!(
            (failed.chars().count(), field, status, codes),
            (78, None, 500, MessageCodes::HaltedH4)
        );
        Ok(())
    }

    // audit: COPAUS0C-016
    // audit: COPAUS0C-017
    // audit: COPAUS0C-018
    // audit: COPAUS0C-019
    #[test]
    fn the_first_failed_master_read_ends_the_request() -> TestResult {
        let (account, customer) = (account(0, 0)?, customer()?);
        let run = |a, c| shown(summary_block(keys()?, a, c, Lookup::NotFound));
        let (text, field, status, _) = run(Lookup::NotFound, Lookup::Found(&customer))?;
        assert!(
            text.starts_with("Account:00000000050 not found in ACCT file. Resp:"),
            "COPAUS0C-016"
        );
        assert_eq!((text.chars().count(), field, status), (73, None, 404));
        let (text, _, status, _) = run(Lookup::Failed, Lookup::Found(&customer))?;
        assert!(
            text.starts_with("Account:00000000050 System error while reading ACCT file. Resp:"),
            "COPAUS0C-017"
        );
        assert_eq!(
            (text.chars().count(), status, text.ends_with(" Reas:")),
            (78, 500, true)
        );
        let (text, _, status, _) = run(Lookup::Found(&account), Lookup::NotFound)?;
        assert!(
            text.starts_with("Customer:000000050 not found in CUST file. Resp:"),
            "COPAUS0C-018"
        );
        assert_eq!((text.chars().count(), status), (72, 404));
        let (text, _, status, codes) = run(Lookup::Found(&account), Lookup::Failed)?;
        assert!(
            text.starts_with("Customer:000000050 System error while reading CUST file. Resp:"),
            "COPAUS0C-019"
        );
        assert_eq!(
            (text.chars().count(), status, codes),
            (78, 500, MessageCodes::HaltedH4)
        );
        Ok(())
    }

    // audit: COPAUS0C-020
    // audit: COPAUS0C-021
    // audit: COPAUS0C-022
    // audit: COPAUS0C-023
    // audit: COPAUS0C-024
    // audit: COPAUS0C-025
    // audit: COPAUS0C-027
    // audit: COPAUS0C-028
    #[test]
    fn the_block_shows_the_customer_limits_and_summary() -> TestResult {
        let acct = PicU::new(50)?;
        assert_eq!(summary_key(acct), PicS::<11>::from(acct));
        let mut summary = PendingAuthSummaryRecord::initialized(summary_key(acct), PicU::new(50)?);
        summary.approved_auth_cnt = Count::new(1);
        summary.approved_auth_amt = Money::new(Decimal::new(2_500, 2))?;
        summary.declined_auth_cnt = Count::new(1_234);
        summary.credit_balance = Money::new(Decimal::new(-525, 2))?;
        summary.declined_auth_amt = Money::new(Decimal::new(12_345_678, 2))?;
        let block = block_for(Some(&summary))?;
        assert_eq!(block.customer_id, "000000050");
        assert_eq!(block.name, "ANN L DOE                ");
        assert_eq!(block.address_line_1, "12 MAIN STREET,BLDG 7    ");
        assert_eq!(block.address_line_2, "SPRINGFIELD,IL,62701     ");
        assert_eq!(block.phone, "(217)555-0100");
        assert_eq!(
            (block.credit_limit.as_str(), block.cash_limit.as_str()),
            ("     1234.56", " 45678.90")
        );
        assert_eq!(block.summary, SummaryState::Found);
        assert_eq!(
            (block.approved_count.as_str(), block.declined_count.as_str()),
            ("001", "234")
        );
        assert_eq!(
            (block.credit_balance.as_str(), block.cash_balance.as_str()),
            ("-       5.25", "     0.00")
        );
        assert_eq!(
            (
                block.approved_amount.as_str(),
                block.declined_amount.as_str()
            ),
            ("    25.00 ", " 23456.78 ")
        );
        summary.approved_auth_cnt = Count::new(-5);
        assert_eq!(block_for(Some(&summary))?.approved_count, "005");
        Ok(())
    }

    // audit: COPAUS0C-029
    // audit: COPAUS0C-030
    // audit: COPAUS0C-037
    // audit: COPAUS0C-038
    #[test]
    fn without_a_summary_the_figures_are_zero_filled() -> TestResult {
        let block = block_for(None)?;
        assert_eq!(block.summary, SummaryState::NotFound);
        assert_eq!(
            (block.approved_count.as_str(), block.declined_count.as_str()),
            ("000", "000")
        );
        assert_eq!(
            (block.credit_balance.as_str(), block.cash_balance.as_str()),
            ("000000000000", "000000000")
        );
        assert_eq!(
            (
                block.approved_amount.as_str(),
                block.declined_amount.as_str()
            ),
            ("0000000000", "0000000000")
        );
        assert_eq!(block.credit_limit, "     1234.56");
        let failed = summary_block(
            keys()?,
            Lookup::Found(&account(0, 0)?),
            Lookup::Found(&customer()?),
            Lookup::Failed,
        );
        let (text, field, status, codes) = shown(failed)?;
        assert!(
            text.starts_with(" System error while reading AUTH Summary: Code:"),
            "COPAUS0C-030"
        );
        assert_eq!(
            (text.chars().count(), field, status, codes),
            (49, None, 500, MessageCodes::HaltedH4)
        );
        let first = detail_failure(PageRequest::First);
        assert!(
            first
                .message()
                .starts_with(" System error while reading AUTH Details: Code:"),
            "COPAUS0C-037"
        );
        assert_eq!(
            (first.message().chars().count(), first.class().http_status()),
            (49, 500)
        );
        let repos = detail_failure(PageRequest::After(key_2022_01_15()?));
        assert!(
            repos
                .message()
                .starts_with(" System error while repos. AUTH Details: Code:"),
            "COPAUS0C-038"
        );
        assert_eq!(repos.message().chars().count(), 48);
        Ok(())
    }

    // audit: COPAUS0C-032
    // audit: COPAUS0C-033
    // audit: COPAUS0C-034
    #[test]
    fn a_row_shows_the_detail_as_stored() -> TestResult {
        let row = row_from(&detail(
            key_2022_01_15()?,
            ("240131", "103045"),
            ("00", "0000"),
            2_500,
        )?);
        assert_eq!(row.key, key_2022_01_15()?);
        assert_eq!(
            (row.transaction_id.as_str(), row.auth_type.as_str()),
            ("TXN000000000001 ", "SALE")
        );
        assert_eq!(
            (row.date.as_str(), row.time.as_str()),
            ("01/31/24", "10:30:45")
        );
        assert_eq!(
            (row.approval.as_str(), row.match_status.as_str()),
            ("A", "P")
        );
        assert_eq!(row.amount, "       25.00");
        let mut declined = detail(key_2022_01_15()?, ("241399", "2 3 4 "), ("05", "4100"), 0)?;
        declined.match_status = PicX::from_text("D")?;
        let row = row_from(&declined);
        assert_eq!(
            (row.date.as_str(), row.time.as_str()),
            ("13/99/24", "2 :3 :4 ")
        );
        assert_eq!(
            (
                row.approval.as_str(),
                row.match_status.as_str(),
                row.amount.as_str()
            ),
            ("D", "D", "        0.00")
        );
        let large = detail(
            key_2022_01_15()?,
            ("220718", "103000"),
            ("00", "0000"),
            123_456_789_012,
        )?;
        assert_eq!(row_from(&large).amount, " 34567890.12");
        Ok(())
    }

    // audit: COPAUS0C-036
    // audit: COPAUS0C-041
    // audit: COPAUS0C-047
    #[test]
    fn pages_hold_five_rows_and_report_the_next_flag() -> TestResult {
        assert_eq!(DEFAULT_PAGE_SIZE, 5);
        let all = details(12)?;
        let first = page(PageRequest::First, &all, DEFAULT_PAGE_SIZE);
        assert_eq!((first.rows.len(), first.next_page), (5, NextPage::Yes));
        let last = first.last_key().ok_or("an empty first page")?;
        assert_eq!(
            PageRequest::new(Some(Direction::Forward), Some(last)),
            PageRequest::After(last)
        );
        let position = all
            .iter()
            .position(|record| AuthKey::of(record) == last)
            .ok_or("no last key")?;
        let after = all.get(position..).ok_or("no rows after the key")?;
        let second = page(PageRequest::After(last), after, DEFAULT_PAGE_SIZE);
        assert_eq!(
            second.rows.first().map(|row| row.key),
            all.get(5).map(AuthKey::of)
        );
        assert_eq!(second.next_page, NextPage::Yes);
        let tail = all.get(10..).ok_or("no tail")?;
        let short = page(PageRequest::First, tail, DEFAULT_PAGE_SIZE);
        assert_eq!(
            (short.rows.len(), short.next_page),
            (2, NextPage::Unreported)
        );
        let exact = page(
            PageRequest::First,
            all.get(..5).ok_or("no head")?,
            DEFAULT_PAGE_SIZE,
        );
        assert_eq!(exact.next_page, NextPage::No);
        let start = first.first_key().ok_or("an empty first page")?;
        assert_eq!(
            PageRequest::new(Some(Direction::Backward), Some(start)),
            PageRequest::From(start)
        );
        let back = page(PageRequest::From(start), &all, DEFAULT_PAGE_SIZE);
        assert_eq!(back.first_key(), Some(start));
        let gone = page(
            PageRequest::From(key_2022_01_15()?),
            &all,
            DEFAULT_PAGE_SIZE,
        );
        assert_eq!((gone.rows.len(), gone.next_page), (0, NextPage::Unreported));
        let rendered = start.render();
        assert_eq!(
            (rendered.len(), AuthKey::parse(&rendered)?),
            (AuthKey::RENDERED_WIDTH, start)
        );
        Ok(())
    }

    // audit: COPAUS0C-055
    #[test]
    fn no_row_or_block_holds_a_card_number() -> TestResult {
        let block = block_for(None)?;
        let row = row_from(&detail(
            key_2022_01_15()?,
            ("240131", "103045"),
            ("00", "0000"),
            2_500,
        )?);
        let texts = [
            block.customer_id,
            block.name,
            block.address_line_1,
            block.address_line_2,
            block.phone,
            row.transaction_id,
            row.auth_type,
            row.amount,
        ];
        assert!(
            texts
                .iter()
                .all(|text| !text.contains(CARD) && !text.contains("5740")),
            "COPAUS0C-055"
        );
        Ok(())
    }
}

mod copaus1c {
    use domain::copybook::{PicU, PicX};
    use domain::online::copaus0c::{Approval, AuthKey, Lookup};
    use domain::online::copaus1c::{
        ALREADY_AT_LAST, AuthDetailFailure, AuthDetailView, DECLINE_REASONS, DetailLookup,
        FraudToggleResult, MessageCodes, NextLookup, SegmentUpdate, detail, edit, fraud_customer,
        next, reason_text, toggle, toggle_outcome,
    };
    use domain::online::copaus2c::{FraudAction, FraudOutcome};
    use domain::records::PendingAuthDetailRecord;

    use super::TestResult;
    use super::authorization::{detail as auth_detail, key_2022_01_15};

    type Outcome<T> = Result<T, Box<dyn std::error::Error>>;

    fn shown<T>(result: Result<T, AuthDetailFailure>) -> Outcome<(String, u16, MessageCodes)> {
        let failure = result.err().ok_or("the step passed")?;
        Ok((
            failure.message().to_owned(),
            failure.class().http_status(),
            failure.codes(),
        ))
    }

    fn stored() -> Outcome<PendingAuthDetailRecord> {
        auth_detail(
            key_2022_01_15()?,
            ("240131", "103045"),
            ("00", "0000"),
            123_456,
        )
    }

    fn view_of(record: &PendingAuthDetailRecord) -> Outcome<AuthDetailView> {
        Ok(detail(PicU::new(50)?, DetailLookup::Found(record))?)
    }

    // audit: COPAUS1C-006
    // audit: COPAUS1C-008
    // audit: COPAUS1C-009
    #[test]
    fn the_request_names_an_account_and_a_decodable_key() -> TestResult {
        let key = key_2022_01_15()?.render();
        for (acct, auth_key) in [
            ("50", key.as_str()),
            ("0000000005O", key.as_str()),
            ("00000000050", "                "),
            ("00000000050", ""),
        ] {
            assert_eq!(
                shown(edit(acct, auth_key))?,
                (String::new(), 400, MessageCodes::None)
            );
        }
        assert_eq!(
            shown(edit("00000000050", "NOT-A-KEY"))?,
            (String::new(), 404, MessageCodes::None)
        );
        let named = edit("00000000050", &key)?;
        assert_eq!(
            (named.acct_id(), named.auth_key()),
            (PicU::new(50)?, key_2022_01_15()?)
        );
        assert_eq!(
            shown(detail(PicU::new(50)?, DetailLookup::NotFound))?,
            (String::new(), 404, MessageCodes::None)
        );
        Ok(())
    }

    // audit: COPAUS1C-012
    // audit: COPAUS1C-013
    // audit: COPAUS1C-014
    // audit: COPAUS1C-015
    // audit: COPAUS1C-016
    // audit: COPAUS1C-019
    // audit: COPAUS1C-020
    // audit: COPAUS1C-021
    // audit: COPAUS1C-023
    // audit: COPAUS1C-035
    #[test]
    fn a_found_detail_shows_every_field_as_the_program_moves_it() -> TestResult {
        let record = stored()?;
        let view = view_of(&record)?;
        assert_eq!(
            (view.acct_id, view.key),
            (PicU::new(50)?, AuthKey::of(&record))
        );
        assert!(
            view.card_num.to_string() == "************5740",
            "COPAUS1C-012"
        );
        assert_eq!(
            (view.auth_date.as_str(), view.auth_time.as_str()),
            ("01/31/24  ", "10:30:45  ")
        );
        assert_eq!(
            (view.response, view.response.as_str()),
            (Approval::Approved, "A")
        );
        assert_eq!(
            (view.auth_code.as_str(), view.amount.as_str()),
            ("001234", "     1234.56")
        );
        assert_eq!(
            (view.pos_entry_mode.as_str(), view.source.as_str()),
            ("05  ", "POS       ")
        );
        assert_eq!(
            (
                view.merchant_category_code.as_str(),
                view.card_expiry.as_str()
            ),
            ("5411", "09/27")
        );
        assert_eq!(
            (view.auth_type.as_str(), view.transaction_id.as_str()),
            ("SALE          ", "TXN000000000001")
        );
        assert_eq!(
            (view.match_status.as_str(), view.merchant_id.as_str()),
            ("P", "MERCHANT0000001")
        );
        assert_eq!(view.merchant_name, format!("{:<25}", "CORNER GROCERY"));
        assert_eq!(view.merchant_city, format!("{:<25}", "SEATTLE"));
        assert_eq!(
            (view.merchant_state.as_str(), view.merchant_zip.as_str()),
            ("WA", "98101     ")
        );
        assert_eq!(view.message, None);
        let odd = auth_detail(
            key_2022_01_15()?,
            ("241399", "256199"),
            ("05", "4100"),
            -100,
        )?;
        let view = view_of(&odd)?;
        assert_eq!(
            (view.auth_date.as_str(), view.auth_time.as_str()),
            ("13/99/24  ", "25:61:99  ")
        );
        assert_eq!(
            (view.response.as_str(), view.amount.as_str()),
            ("D", "-       1.00")
        );
        Ok(())
    }

    // audit: COPAUS1C-017
    // audit: COPAUS1C-018
    // audit: COPAUS1C-022
    #[test]
    fn the_reason_and_fraud_status_are_looked_up_and_formatted() -> TestResult {
        let codes: Vec<&str> = DECLINE_REASONS.iter().map(|(code, _)| *code).collect();
        assert_eq!(
            codes,
            [
                "0000", "3100", "4100", "4200", "4300", "4400", "5100", "5200", "5300", "9000"
            ]
        );
        for (code, text) in [
            ("0000", "0000-APPROVED"),
            ("3100", "3100-INVALID CARD"),
            ("4100", "4100-INSUFFICNT FUND"),
            ("4300", "4300-ACCOUNT CLOSED"),
            ("5300", "5300-LOST CARD"),
            ("9000", "9000-UNKNOWN"),
            ("1234", "9999-ERROR"),
            ("    ", "9999-ERROR"),
        ] {
            assert_eq!(
                reason_text(&PicX::from_text(code)?),
                format!("{text:<20}"),
                "{code}"
            );
        }
        for (flag, status) in [
            ("F", "F-07/18/22"),
            ("R", "R-07/18/22"),
            (" ", "-         "),
            ("X", "-         "),
        ] {
            let mut record = stored()?;
            record.auth_fraud = PicX::from_text(flag)?;
            record.fraud_rpt_date = PicX::from_text("07/18/22")?;
            assert_eq!(view_of(&record)?.fraud_status, status, "{flag:?}");
        }
        Ok(())
    }

    // audit: COPAUS1C-010
    // audit: COPAUS1C-011
    // audit: COPAUS1C-024
    // audit: COPAUS1C-025
    // audit: COPAUS1C-026
    #[test]
    fn reads_and_f8_answer_with_the_detail_or_their_message() -> TestResult {
        let acct = PicU::new(50)?;
        let (text, status, codes) = shown(detail(acct, DetailLookup::SummaryError))?;
        assert!(
            text.starts_with(" System error while reading Auth Summary: Code:"),
            "COPAUS1C-010"
        );
        assert_eq!(
            (text.chars().count(), status, codes),
            (49, 500, MessageCodes::HaltedH4)
        );
        let (text, status, _) = shown(detail(acct, DetailLookup::DetailError))?;
        assert!(
            text.starts_with(" System error while reading Auth Details: Code:"),
            "COPAUS1C-011"
        );
        assert_eq!((text.chars().count(), status), (49, 500));
        let current = stored()?;
        let following = auth_detail(
            AuthKey::new(
                current.auth_date_9c,
                domain::copybook::PicS::new(896_954_877)?,
            ),
            ("240131", "103044"),
            ("05", "4100"),
            0,
        )?;
        let view = next(acct, &current, NextLookup::Found(&following))?;
        assert_eq!(
            (view.key, view.response, view.reason.as_str()),
            (
                AuthKey::of(&following),
                Approval::Declined,
                "4100-INSUFFICNT FUND"
            )
        );
        assert_eq!((view.amount.as_str(), view.message), ("        0.00", None));
        let end = next(acct, &current, NextLookup::End)?;
        assert_eq!(
            (end.key, end.message),
            (AuthKey::of(&current), Some(ALREADY_AT_LAST))
        );
        assert_eq!(ALREADY_AT_LAST, "Already at the last Authorization...");
        let (text, status, _) = shown(next(acct, &current, NextLookup::Error))?;
        assert!(
            text.starts_with(" System error while reading next Auth: Code:"),
            "COPAUS1C-026"
        );
        assert_eq!((text.chars().count(), status), (46, 500));
        Ok(())
    }

    // audit: COPAUS1C-028
    // audit: COPAUS1C-030
    #[test]
    fn f5_flips_the_stored_flag_for_the_cross_reference_customer() -> TestResult {
        for (flag, action, flipped) in [
            ("F", FraudAction::Remove, "R"),
            ("R", FraudAction::Report, "F"),
            (" ", FraudAction::Report, "F"),
        ] {
            let mut record = stored()?;
            record.auth_fraud = PicX::from_text(flag)?;
            let flip = toggle(&record);
            assert_eq!(
                (flip.action, flip.updated.auth_fraud.to_text()),
                (action, flipped.to_owned()),
                "{flag:?}"
            );
            assert_eq!(
                PendingAuthDetailRecord {
                    auth_fraud: record.auth_fraud.clone(),
                    ..flip.updated
                },
                record
            );
        }
        let cust = PicU::new(50)?;
        assert_eq!(fraud_customer(Lookup::Found(cust))?, cust);
        assert_eq!(shown(fraud_customer(Lookup::NotFound))?.1, 404);
        assert_eq!(shown(fraud_customer(Lookup::Failed))?.1, 500);
        Ok(())
    }

    // audit: COPAUS1C-031
    // audit: COPAUS1C-033
    #[test]
    fn a_failed_fraud_write_or_replace_rolls_back() -> TestResult {
        let acct = PicU::new(50)?;
        let flip = toggle(&stored()?);
        for (outcome, prefix) in [
            (FraudOutcome::InsertFailed, " SYSTEM ERROR DB2: CODE:"),
            (FraudOutcome::UpdateFailed, " UPDT ERROR DB2: CODE:"),
        ] {
            let result = toggle_outcome(acct, &flip, outcome, None);
            assert!(!result.commits(), "COPAUS1C-031");
            assert!(result.message().starts_with(prefix), "COPAUS1C-031");
            assert!(
                matches!(result, FraudToggleResult::RolledBack(_)),
                "COPAUS1C-031"
            );
        }
        let replace_failed = toggle_outcome(
            acct,
            &flip,
            FraudOutcome::Added,
            Some(SegmentUpdate::Failed),
        );
        assert!(!replace_failed.commits(), "COPAUS1C-033");
        assert!(
            replace_failed
                .message()
                .starts_with(" System error while FRAUD Tagging, ROLLBACK||"),
            "COPAUS1C-033"
        );
        assert_eq!(shown(replace_failed.into_result())?.1, 500);
        Ok(())
    }

    /// HALTED(X-COPAUS1C-H2): the committed branch follows a replace after a non-hold read; it
    /// runs here for coverage only and no value is asserted.
    #[test]
    fn the_committed_toggle_runs_without_parity_assertions() -> TestResult {
        let flip = toggle(&stored()?).with_report_date(PicX::from_text("07/18/22")?);
        let _ = toggle_outcome(
            PicU::new(50)?,
            &flip,
            FraudOutcome::Updated,
            Some(SegmentUpdate::Replaced),
        )
        .into_result();
        Ok(())
    }
}

mod copaus2c {
    use domain::copybook::{PicS, PicU, PicX};
    use domain::online::copaus0c::AuthKey;
    use domain::online::copaus2c::{
        ACT_MSG_WIDTH, FraudAction, FraudOutcome, InsertResult, STATE_SEPARATOR, UpdateResult,
        fraud_outcome, fraud_row, report_date, with_report_date,
    };

    use super::authorization::{CARD, detail, key_2022_01_15};
    use super::{TestResult, at, clock, date, has_layout};

    // audit: COPAUS2C-001
    #[test]
    fn the_report_date_is_mm_dd_yy_with_slashes() -> TestResult {
        let today = report_date(&clock()?);
        assert!(has_layout(today.as_bytes(), b"99/99/99"), "COPAUS2C-001");
        let record = detail(
            key_2022_01_15()?,
            ("220115", "103045"),
            ("00", "0000"),
            2_500,
        )?;
        assert_eq!(
            with_report_date(&record, today.clone()).fraud_rpt_date,
            today
        );
        Ok(())
    }

    // audit: COPAUS2C-002
    // audit: COPAUS2C-003
    // audit: COPAUS2C-004
    // audit: COPAUS2C-006
    #[test]
    fn the_fraud_row_rebuilds_the_authorization_timestamp() -> TestResult {
        let record = detail(
            key_2022_01_15()?,
            ("220115", "103045"),
            ("00", "0000"),
            2_500,
        )?;
        let row = fraud_row(
            &record,
            FraudAction::Report,
            PicU::new(50)?,
            PicU::new(50)?,
            &clock()?,
        )?;
        let expected = at(date(2022, 1, 15)?, 10, 30, 45)?
            .checked_add_signed(chrono::TimeDelta::milliseconds(123))
            .ok_or("no timestamp")?;
        assert_eq!(row.auth_ts, expected);
        assert!(row.card_num.expose().to_text() == CARD, "COPAUS2C-006");
        assert_eq!(
            (row.auth_type.clone(), row.card_expiry_date.clone()),
            (record.auth_type.clone(), record.card_expiry_date.clone())
        );
        assert_eq!(
            (row.merchant_id.clone(), row.transaction_id.clone()),
            (record.merchant_id.clone(), record.transaction_id.clone())
        );
        assert_eq!(
            (row.approved_amt, row.transaction_amt),
            (record.approved_amt, record.transaction_amt)
        );
        assert_eq!(row.processing_code.to_text(), "001234");
        assert_eq!(
            (row.auth_fraud.to_text(), row.acct_id, row.cust_id),
            ("F".to_owned(), PicU::new(50)?, PicU::new(50)?)
        );
        let other = detail(
            AuthKey::new(PicS::new(77_984)?, PicS::new(764_040_000)?),
            ("220115", "235959"),
            ("00", "0000"),
            0,
        )?;
        let late = fraud_row(
            &other,
            FraudAction::Remove,
            PicU::new(50)?,
            PicU::new(50)?,
            &clock()?,
        )?;
        assert_eq!(late.auth_ts.date(), date(2022, 1, 15)?);
        assert_eq!(late.auth_fraud.to_text(), "R");
        Ok(())
    }

    // audit: COPAUS2C-005
    // audit: COPAUS2C-010
    #[test]
    fn a_row_that_cannot_be_built_takes_the_insert_failure() -> TestResult {
        let bad = detail(
            key_2022_01_15()?,
            ("221399", "103045"),
            ("00", "0000"),
            2_500,
        )?;
        let error = fraud_row(
            &bad,
            FraudAction::Report,
            PicU::new(50)?,
            PicU::new(50)?,
            &clock()?,
        )
        .err()
        .ok_or("13/99 built a row")?;
        assert_eq!(FraudOutcome::from(error), FraudOutcome::InsertFailed);
        let message = FraudOutcome::InsertFailed.message();
        assert_eq!((message.as_bytes().len(), ACT_MSG_WIDTH), (50, 50));
        assert_eq!(
            message.as_bytes().get(..24),
            Some(&b" SYSTEM ERROR DB2: CODE:"[..])
        );
        assert_eq!(
            message.as_bytes().get(31..40),
            Some(STATE_SEPARATOR.as_bytes())
        );
        assert_eq!(
            fraud_outcome(InsertResult::Failed, Some(UpdateResult::Updated)),
            FraudOutcome::InsertFailed
        );
        assert!(!FraudOutcome::InsertFailed.is_success(), "COPAUS2C-010");
        Ok(())
    }

    // audit: COPAUS2C-007
    // audit: COPAUS2C-008
    // audit: COPAUS2C-009
    // audit: COPAUS2C-011
    // audit: COPAUS2C-013
    #[test]
    fn the_insert_and_update_outcomes_carry_their_texts() -> TestResult {
        let added = fraud_outcome(InsertResult::Inserted, None);
        assert_eq!((added, added.is_success()), (FraudOutcome::Added, true));
        assert_eq!(added.message(), PicX::from_text("ADD SUCCESS")?);
        assert!(
            InsertResult::Duplicate.needs_update() && !InsertResult::Inserted.needs_update(),
            "COPAUS2C-008"
        );
        let updated = fraud_outcome(InsertResult::Duplicate, Some(UpdateResult::Updated));
        assert_eq!(
            (updated, updated.is_success()),
            (FraudOutcome::Updated, true)
        );
        assert_eq!(updated.message(), PicX::from_text("UPDT SUCCESS")?);
        let failed = fraud_outcome(InsertResult::Duplicate, Some(UpdateResult::Failed));
        assert_eq!(
            (failed, failed.is_success()),
            (FraudOutcome::UpdateFailed, false)
        );
        assert_eq!(
            failed.message().as_bytes().get(..22),
            Some(&b" UPDT ERROR DB2: CODE:"[..])
        );
        assert_eq!(
            failed.message().as_bytes().get(29..38),
            Some(STATE_SEPARATOR.as_bytes())
        );
        assert_eq!(
            fraud_outcome(InsertResult::Duplicate, None),
            FraudOutcome::UpdateFailed
        );
        for outcome in [
            FraudOutcome::Added,
            FraudOutcome::Updated,
            FraudOutcome::InsertFailed,
            FraudOutcome::UpdateFailed,
        ] {
            let text = outcome.message().to_text();
            assert!(
                !text.contains(CARD) && !text.contains("5740"),
                "COPAUS2C-013"
            );
        }
        Ok(())
    }
}

mod cotrtupc {
    use domain::copybook::PicX;
    use domain::online::cotrtupc::{
        MaintEffect, MaintFailure, MaintField, MaintInput, MaintOutcome, StoreResult, TranTypeView,
        TtupAid, TtupState, TypeLookup, apply_result, check_pfkeys, decide, edit_description,
        edit_type_code, fetch, fetch_key, receive_type_code,
    };
    use domain::records::Db2TransactionType;

    use super::TestResult;

    type Outcome<T> = Result<T, Box<dyn std::error::Error>>;

    fn purchase() -> Outcome<Db2TransactionType> {
        Ok(Db2TransactionType::new(
            PicX::from_text("01")?,
            "PURCHASE".to_owned(),
        )?)
    }

    fn padded(text: &str) -> String {
        format!("{text:<50}")
    }

    fn failure(outcome: MaintOutcome) -> Outcome<MaintFailure> {
        match outcome {
            MaintOutcome::Failed(failure) => Ok(failure),
            other => Err(format!("expected a failure, got {other:?}").into()),
        }
    }

    fn screen(outcome: MaintOutcome) -> Outcome<TranTypeView> {
        match outcome {
            MaintOutcome::Screen(view) => Ok(view),
            other => Err(format!("expected a screen, got {other:?}").into()),
        }
    }

    fn input_failure<T>(
        result: Result<T, MaintFailure>,
    ) -> Outcome<(&'static str, Option<MaintField>, u16)> {
        let failure = result.err().ok_or("the edit passed")?;
        Ok((
            failure.message(),
            failure.field(),
            failure.class().http_status(),
        ))
    }

    // audit: COTRTUPC-004
    // audit: COTRTUPC-005
    // audit: COTRTUPC-006
    // audit: COTRTUPC-007
    // audit: COTRTUPC-008
    #[test]
    fn the_type_code_is_stored_edited_and_normalised() -> TestResult {
        assert_eq!(receive_type_code(" 9")?, PicX::from_text("9 ")?);
        assert_eq!(receive_type_code("9")?, PicX::from_text("9 ")?);
        for raw in ["", "*", "  "] {
            assert_eq!(
                input_failure(edit_type_code(raw))?,
                (
                    "Tran Type code must be supplied.",
                    Some(MaintField::TypeCode),
                    400
                )
            );
        }
        for raw in ["AB", "1A", "-", ".", "CR"] {
            assert_eq!(
                input_failure(edit_type_code(raw))?.0,
                "Tran Type code must be numeric.",
                "{raw}"
            );
        }
        for raw in ["00", "0", "-0", "0.", ".0"] {
            assert_eq!(
                input_failure(edit_type_code(raw))?.0,
                "Tran Type code must not be zero.",
                "{raw}"
            );
        }
        for raw in ["1", "01", "+1", "-1", "1-", "1."] {
            assert_eq!(edit_type_code(raw)?, PicX::from_text("01")?, "{raw}");
        }
        assert_eq!(edit_description("  Purchase  ")?, padded("Purchase"));
        assert_eq!(
            input_failure(edit_description("*"))?,
            (
                "Transaction Desc must be supplied.",
                Some(MaintField::Description),
                400
            )
        );
        Ok(())
    }

    // audit: COTRTUPC-009
    // audit: COTRTUPC-010
    // audit: COTRTUPC-011
    // audit: COTRTUPC-012
    #[test]
    fn the_fetch_shows_the_row_or_offers_the_add() -> TestResult {
        let row = purchase()?;
        let key = edit_type_code("1")?;
        let found = fetch(&key, TypeLookup::Found(&row))?;
        assert_eq!(
            (found.state(), found.code(), found.description()),
            (TtupState::ShowDetails, &key, Some("PURCHASE"))
        );
        assert_eq!(
            (found.message(), found.info_message()),
            (None, "Enter transaction type to be maintained")
        );
        let missing = fetch(&key, TypeLookup::NotFound)?;
        assert_eq!(
            (missing.state(), missing.description()),
            (TtupState::DetailsNotFound, None)
        );
        assert_eq!(
            missing.message(),
            Some("No record found for this key in database")
        );
        assert_eq!(missing.info_message(), "Press F05 to add. F12 to cancel");
        let error = fetch(&key, TypeLookup::Error)
            .err()
            .ok_or("a failed read showed a row")?;
        assert!(
            error
                .message()
                .starts_with("Error accessing: TRANSACTION_TYPE table. SQLCODE:"),
            "COTRTUPC-011"
        );
        assert_eq!(error.class().http_status(), 500);
        assert_eq!(
            fetch_key("AB", TtupState::DetailsNotFound)?,
            PicX::from_text("AB")?
        );
        assert_eq!(
            input_failure(fetch_key("AB", TtupState::DetailsNotFetched))?.0,
            "Tran Type code must be numeric."
        );
        assert_eq!(
            fetch_key(" 5", TtupState::DetailsNotFetched)?,
            PicX::from_text("05")?
        );
        Ok(())
    }

    // audit: COTRTUPC-016
    // audit: COTRTUPC-017
    // audit: COTRTUPC-018
    // audit: COTRTUPC-019
    // audit: COTRTUPC-020
    // audit: COTRTUPC-021
    #[test]
    fn enter_edits_the_change_against_the_stored_row() -> TestResult {
        let row = purchase()?;
        let found = TypeLookup::Found(&row);
        let enter = |type_code, description, state, current| {
            decide(
                TtupAid::Enter,
                state,
                &MaintInput {
                    type_code,
                    description,
                },
                current,
            )
        };
        let bad_key = failure(enter("AB", "Purchases", TtupState::ShowDetails, found))?;
        assert_eq!(bad_key.message(), "Tran Type code must be numeric.");
        let same = failure(enter("01", "  purchase ", TtupState::ShowDetails, found))?;
        assert_eq!(
            (same.message(), same.field(), same.state()),
            (
                "No change detected with respect to values fetched.",
                Some(MaintField::Description),
                TtupState::ShowDetails
            )
        );
        let empty = failure(enter("01", "*", TtupState::ShowDetails, found))?;
        assert_eq!(
            (empty.message(), empty.field()),
            (
                "Transaction Desc must be supplied.",
                Some(MaintField::Description)
            )
        );
        let symbol = failure(enter("01", "Buy-now", TtupState::ShowDetails, found))?;
        assert_eq!(
            (symbol.message(), symbol.state()),
            (
                "Transaction Desc can have numbers or alphabets only.",
                TtupState::ChangesNotOk
            )
        );
        let valid = screen(enter("01", "Purchases 2", TtupState::ShowDetails, found))?;
        assert_eq!(
            (valid.state(), valid.message()),
            (TtupState::ChangesOkNotConfirmed, None)
        );
        assert_eq!(valid.info_message(), "Changes validated.Press F5 to save");
        let new = screen(enter(
            "42",
            "New type",
            TtupState::CreateNewRecord,
            TypeLookup::NotFound,
        ))?;
        assert_eq!(
            (new.state(), new.code()),
            (TtupState::ChangesOkNotConfirmed, &PicX::from_text("42")?)
        );
        Ok(())
    }

    // audit: COTRTUPC-023
    // audit: COTRTUPC-024
    // audit: COTRTUPC-026
    // audit: COTRTUPC-027
    // audit: COTRTUPC-028
    // audit: COTRTUPC-029
    #[test]
    fn f5_saves_by_update_then_insert_and_reports_the_store_result() -> TestResult {
        let row = purchase()?;
        let input = MaintInput {
            type_code: "01",
            description: "  New desc  ",
        };
        let MaintOutcome::Execute(update) = decide(
            TtupAid::Pf5,
            TtupState::ChangesOkNotConfirmed,
            &input,
            TypeLookup::Found(&row),
        ) else {
            return Err("COTRTUPC-023: F5 wrote nothing".into());
        };
        let code = PicX::from_text("01")?;
        assert_eq!(
            update,
            MaintEffect::Update {
                code: code.clone(),
                description: padded("New desc")
            }
        );
        let done = screen(apply_result(update.clone(), StoreResult::Done))?;
        assert_eq!(
            (done.state(), done.info_message()),
            (
                TtupState::ChangesOkayedAndDone,
                "Changes committed to database"
            )
        );
        let insert = MaintEffect::Insert {
            code: code.clone(),
            description: padded("New desc"),
        };
        assert_eq!(
            apply_result(update.clone(), StoreResult::NoRow),
            MaintOutcome::Execute(insert.clone())
        );
        assert_eq!(
            screen(apply_result(insert.clone(), StoreResult::Done))?.state(),
            TtupState::ChangesOkayedAndDone
        );
        let not_inserted = failure(apply_result(insert, StoreResult::OtherError))?;
        assert!(
            not_inserted
                .message()
                .starts_with("Error inserting record into: TRANSACTION_TYPE Table. SQLCODE:"),
            "COTRTUPC-027"
        );
        assert_eq!(
            (
                not_inserted.class().http_status(),
                not_inserted.state().info_message()
            ),
            (500, "Changes committed to database")
        );
        let not_updated = failure(apply_result(update.clone(), StoreResult::OtherError))?;
        assert!(
            not_updated
                .message()
                .starts_with("Error updating: TRANSACTION_TYPE Table. SQLCODE:"),
            "COTRTUPC-028"
        );
        let locked = failure(apply_result(update, StoreResult::from_sqlstate("55P03")))?;
        assert_eq!(
            (locked.message(), locked.state()),
            (
                "Could not lock record for update",
                TtupState::ChangesOkayedLockError
            )
        );
        assert_eq!(
            (
                locked.state().info_message(),
                StoreResult::from_sqlstate("40P01")
            ),
            ("Changes unsuccessful", StoreResult::LockNotAvailable)
        );
        Ok(())
    }

    // audit: COTRTUPC-032
    // audit: COTRTUPC-033
    // audit: COTRTUPC-034
    // audit: COTRTUPC-043
    #[test]
    fn the_second_f4_deletes_unless_categories_refer_to_the_type() -> TestResult {
        let row = purchase()?;
        let input = MaintInput {
            type_code: "01",
            description: "",
        };
        let prompt = screen(decide(
            TtupAid::Pf4,
            TtupState::ShowDetails,
            &input,
            TypeLookup::Found(&row),
        ))?;
        assert_eq!(
            (prompt.state(), prompt.info_message()),
            (
                TtupState::ConfirmDelete,
                "Delete this record ? Press F4 to confirm"
            )
        );
        let code = PicX::from_text("01")?;
        let delete = decide(
            TtupAid::Pf4,
            TtupState::ConfirmDelete,
            &input,
            TypeLookup::Found(&row),
        );
        assert_eq!(
            delete,
            MaintOutcome::Execute(MaintEffect::Delete { code: code.clone() })
        );
        let deleted = screen(apply_result(
            MaintEffect::Delete { code: code.clone() },
            StoreResult::Done,
        ))?;
        assert_eq!(
            (deleted.state(), deleted.info_message()),
            (TtupState::DeleteDone, "Delete successful.")
        );
        let restricted = failure(apply_result(
            MaintEffect::Delete { code: code.clone() },
            StoreResult::from_sqlstate("23503"),
        ))?;
        assert!(
            restricted
                .message()
                .starts_with("Please delete associated child records first:SQLCODE :"),
            "COTRTUPC-033"
        );
        assert_eq!(restricted.class().http_status(), 500);
        for result in [StoreResult::NoRow, StoreResult::OtherError] {
            let failed = failure(apply_result(
                MaintEffect::Delete { code: code.clone() },
                result,
            ))?;
            assert!(
                failed
                    .message()
                    .starts_with("Delete failed with message:SQLCODE :"),
                "COTRTUPC-034"
            );
            assert_eq!(
                (failed.state(), failed.state().info_message()),
                (TtupState::DeleteFailed, "Changes unsuccessful")
            );
        }
        let invalid = failure(decide(
            TtupAid::Pf12,
            TtupState::ShowDetails,
            &input,
            TypeLookup::Found(&row),
        ))?;
        assert_eq!(
            (invalid.message(), invalid.class().http_status()),
            ("Invalid key pressed", 400)
        );
        assert_eq!(
            input_failure(check_pfkeys(TtupAid::Pf4, TtupState::DetailsNotFetched))?.0,
            "Invalid key pressed"
        );
        assert_eq!(
            check_pfkeys(TtupAid::Pf5, TtupState::DetailsNotFound)?,
            TtupAid::Pf5
        );
        Ok(())
    }
}

mod cotrtlic {
    use domain::copybook::PicX;
    use domain::online::cotrtlic::{
        CommitEffect, CommitResult, Committed, DEFAULT_PAGE_SIZE, Direction, FilterQuery,
        FilterStatus, InfoMessage, LastPage, ListActionOutcome, ListAid, ListInput, ListMessage,
        NextPage, PageNumber, PageSize, PendingAction, Progress, ReadDirection, ReadQuery,
        RowIndex, RowInput, Selection, ShownRow, StatusClass, TranTypeListState, act, load, page,
    };
    use domain::records::Db2TransactionType;

    use super::TestResult;

    type Outcome<T> = Result<T, Box<dyn std::error::Error>>;

    /// The Db2 seed of `DB2LTTYP.ctl`, which migration 0006 loads.
    const SEED: [(&str, &str); 7] = [
        ("01", "PURCHASE"),
        ("02", "PAYMENT"),
        ("03", "CREDIT"),
        ("04", "AUTHORIZATION"),
        ("05", "REFUND"),
        ("06", "REVERAL"),
        ("07", "ADJUSTMENT"),
    ];

    /// An in-memory `db2.transaction_type`, answering the steps `act`, `load` and `page` name.
    struct Table {
        rows: Vec<Db2TransactionType>,
        /// The codes `db2.transaction_type_category` still refers to (`ON DELETE RESTRICT`).
        parents: Vec<PicX<2>>,
    }

    impl Table {
        fn of(entries: &[(String, String)], parents: usize) -> Outcome<Self> {
            let rows = entries
                .iter()
                .map(|(code, description)| {
                    Ok(Db2TransactionType::new(
                        PicX::from_text(code)?,
                        description.clone(),
                    )?)
                })
                .collect::<Outcome<Vec<_>>>()?;
            let parents = rows
                .iter()
                .take(parents)
                .map(|row| row.tr_type().clone())
                .collect();
            Ok(Self { rows, parents })
        }

        /// The seed, every type but `07` with categories.
        fn seed() -> Outcome<Self> {
            Self::of(
                &SEED.map(|(code, description)| (code.to_owned(), description.to_owned())),
                6,
            )
        }

        /// Types `01` to `count`, described `TYPE n`, none with categories.
        fn numbered(count: u8) -> Outcome<Self> {
            let entries: Vec<(String, String)> = (1..=count)
                .map(|n| (format!("{n:02}"), format!("TYPE {n}")))
                .collect();
            Self::of(&entries, 0)
        }

        fn matches(row: &Db2TransactionType, filters: &FilterQuery) -> bool {
            let text: Vec<char> = row.tr_description().chars().collect();
            filters
                .type_filter()
                .is_none_or(|code| row.tr_type() == code)
                && filters.desc_pattern().is_none_or(|pattern| {
                    let pattern: Vec<char> = pattern.trim_matches(' ').chars().collect();
                    like(&text, &pattern)
                })
        }

        fn read(&self, query: &ReadQuery) -> Vec<Db2TransactionType> {
            let mut rows: Vec<Db2TransactionType> = self
                .rows
                .iter()
                .filter(|row| Self::matches(row, query.filters()))
                .filter(|row| match query.direction() {
                    ReadDirection::Forward => row.tr_type() >= query.start(),
                    ReadDirection::Backward => row.tr_type() < query.start(),
                })
                .cloned()
                .collect();
            rows.sort_by(|a, b| a.tr_type().cmp(b.tr_type()));
            if query.direction() == ReadDirection::Backward {
                rows.reverse();
            }
            rows.truncate(usize::try_from(query.limit()).unwrap_or(0));
            rows
        }

        fn find(&self, code: &PicX<2>) -> Option<Db2TransactionType> {
            self.rows.iter().find(|row| row.tr_type() == code).cloned()
        }

        fn commit(&mut self, effect: &CommitEffect) -> CommitResult {
            let Some(position) = self
                .rows
                .iter()
                .position(|row| row.tr_type() == effect.code())
            else {
                return CommitResult::NotFound;
            };
            match effect {
                CommitEffect::Update { code, description } => {
                    match Db2TransactionType::new(code.clone(), description.clone()) {
                        Ok(updated) => {
                            self.rows.push(updated);
                            self.rows.swap_remove(position);
                            CommitResult::Done
                        }
                        Err(_) => CommitResult::Failed,
                    }
                }
                CommitEffect::Delete { code } if self.parents.contains(code) => {
                    CommitResult::Restricted
                }
                CommitEffect::Delete { .. } => {
                    self.rows.remove(position);
                    CommitResult::Done
                }
            }
        }

        fn description(&self, code: &str) -> Outcome<String> {
            Ok(self
                .find(&PicX::from_text(code)?)
                .ok_or("no such type")?
                .tr_description()
                .to_owned())
        }
    }

    /// SQL `LIKE` with `%` and `_` and no escape character.
    fn like(text: &[char], pattern: &[char]) -> bool {
        match pattern.split_first() {
            None => text.is_empty(),
            Some(('%', rest)) => {
                (0..=text.len()).any(|skip| text.get(skip..).is_some_and(|tail| like(tail, rest)))
            }
            Some(('_', rest)) => text.split_first().is_some_and(|(_, tail)| like(tail, rest)),
            Some((wanted, rest)) => text
                .split_first()
                .is_some_and(|(head, tail)| head == wanted && like(tail, rest)),
        }
    }

    /// The service's loop: each store step the progress names, then the response.
    fn run(table: &mut Table, mut progress: Progress) -> ListActionOutcome {
        loop {
            progress = match progress {
                Progress::Count(step) => {
                    let count = table
                        .rows
                        .iter()
                        .filter(|row| Table::matches(row, step.query()))
                        .count();
                    step.finish(Ok(i64::try_from(count).unwrap_or(i64::MAX)))
                }
                Progress::Verify(step) => {
                    let current = table.find(step.code());
                    step.finish(current.as_ref())
                }
                Progress::Commit(step) => {
                    let result = table.commit(step.effect());
                    step.finish(result)
                }
                Progress::Read(step) => {
                    let rows = table.read(step.query());
                    return step.finish(Ok(rows));
                }
                Progress::Done(outcome) => return *outcome,
            };
        }
    }

    fn loaded(table: &mut Table) -> TranTypeListState {
        run(table, load(PageSize::DEFAULT)).into_state()
    }

    fn request(
        table: &mut Table,
        aid: ListAid,
        state: &TranTypeListState,
        filters: (&str, &str),
        rows: &[RowInput<'_>],
    ) -> ListActionOutcome {
        let input = ListInput {
            type_filter: filters.0,
            desc_filter: filters.1,
            rows,
        };
        run(table, act(aid, &input, state, PageSize::DEFAULT))
    }

    /// Seven rows, blank but for `(row number, selection, description)`.
    fn rows_with(entries: &[(usize, &'static str, &'static str)]) -> Vec<RowInput<'static>> {
        (1..=7)
            .map(|number| {
                entries
                    .iter()
                    .find(|(wanted, ..)| *wanted == number)
                    .map_or_else(RowInput::default, |&(_, selection, description)| RowInput {
                        selection,
                        description,
                    })
            })
            .collect()
    }

    /// The codes shown, `""` at an empty position.
    fn codes(outcome: &ListActionOutcome) -> Vec<String> {
        outcome
            .rows()
            .iter()
            .map(|row| {
                row.as_ref()
                    .map_or_else(String::new, |row| row.code().to_text())
            })
            .collect()
    }

    fn shown(outcome: &ListActionOutcome, number: i64) -> Outcome<&ShownRow> {
        Ok(RowIndex::from_number(number)
            .and_then(|index| outcome.rows().get(index.index()))
            .and_then(Option::as_ref)
            .ok_or("no row at that position")?)
    }

    fn texts(codes: &[&str]) -> Vec<String> {
        codes.iter().map(|code| (*code).to_owned()).collect()
    }

    // audit: COTRTLIC-002
    // audit: COTRTLIC-003
    // audit: COTRTLIC-004
    // audit: COTRTLIC-006
    #[test]
    fn the_first_entry_reads_page_one_and_enter_refreshes_it() -> TestResult {
        assert_eq!((DEFAULT_PAGE_SIZE, PageSize::DEFAULT.rows()), (7, 7));
        let mut table = Table::numbered(9)?;
        let first = run(&mut table, load(PageSize::DEFAULT));
        assert_eq!(
            codes(&first),
            texts(&["01", "02", "03", "04", "05", "06", "07"])
        );
        assert_eq!(
            (first.page_no(), first.next_page(), first.message()),
            (PageNumber::FIRST, NextPage::Exists, None)
        );
        assert_eq!(
            first.info().map(InfoMessage::text),
            Some("Type U to update, D to delete any record")
        );
        assert_eq!(
            (first.status().http_status(), first.state().pending()),
            (200, PendingAction::None)
        );
        assert_eq!(
            (
                first.state().first_key().to_text(),
                first.state().last_key().to_text()
            ),
            ("01".to_owned(), "08".to_owned())
        );
        let empty = run(&mut Table::numbered(0)?, load(PageSize::DEFAULT));
        assert_eq!(
            empty.message_text().as_deref(),
            Some("No records found for this search condition.")
        );
        assert_eq!(
            (empty.info(), empty.next_page()),
            (None, NextPage::NotExists)
        );
        let mut seeded = Table::seed()?;
        let state = loaded(&mut seeded);
        seeded.rows.retain(|row| row.tr_type().to_text() != "03");
        let refreshed = request(&mut seeded, ListAid::Enter, &state, ("", ""), &[]);
        assert_eq!(
            codes(&refreshed),
            texts(&["01", "02", "04", "05", "06", "07", ""])
        );
        assert_eq!(
            (refreshed.committed(), refreshed.info()),
            (Committed::Nothing, Some(InfoMessage::RecordActions))
        );
        Ok(())
    }

    // audit: COTRTLIC-007
    // audit: COTRTLIC-008
    // audit: COTRTLIC-009
    // audit: COTRTLIC-010
    // audit: COTRTLIC-011
    #[test]
    fn f7_and_f8_move_between_pages_with_the_paging_messages() -> TestResult {
        let mut table = Table::numbered(15)?;
        let state = loaded(&mut table);
        let paged = |table: &mut Table, direction, state: &TranTypeListState| {
            run(
                table,
                page(
                    direction,
                    state.type_filter(),
                    state.desc_filter(),
                    state,
                    PageSize::DEFAULT,
                ),
            )
        };
        let second = paged(&mut table, Direction::Forward, &state);
        assert_eq!(
            codes(&second),
            texts(&["08", "09", "10", "11", "12", "13", "14"])
        );
        assert_eq!((second.page_no().get(), second.message()), (2, None));
        let third = paged(&mut table, Direction::Forward, second.state());
        assert_eq!(
            (codes(&third), third.page_no().get()),
            (texts(&["15", "", "", "", "", "", ""]), 3)
        );
        assert_eq!(
            third.message_text().as_deref(),
            Some("No more pages for these search conditions")
        );
        assert_eq!(third.state().last_page(), LastPage::Shown);
        let again = paged(&mut table, Direction::Forward, third.state());
        assert_eq!(
            (again.message_text().as_deref(), again.info()),
            (Some("No more pages to display"), None)
        );
        let back = paged(&mut table, Direction::Backward, third.state());
        assert_eq!(
            (codes(&back), back.page_no().get()),
            (texts(&["08", "09", "10", "11", "12", "13", "14"]), 2)
        );
        let top = paged(&mut table, Direction::Backward, back.state());
        assert_eq!(
            (top.page_no(), top.message_text().as_deref()),
            (PageNumber::FIRST, Some("No previous pages to display"))
        );
        let stay = paged(&mut table, Direction::Backward, top.state());
        assert_eq!(
            (codes(&stay).first().cloned(), stay.message()),
            (Some("01".to_owned()), Some(ListMessage::NoPreviousPages))
        );
        Ok(())
    }

    // audit: COTRTLIC-005
    // audit: COTRTLIC-029
    // audit: COTRTLIC-030
    // audit: COTRTLIC-035
    // audit: COTRTLIC-036
    // audit: COTRTLIC-037
    #[test]
    fn a_changed_filter_with_blank_selections_reads_page_one_of_it() -> TestResult {
        let mut table = Table::seed()?;
        let state = loaded(&mut table);
        for aid in [ListAid::Enter, ListAid::Pf10] {
            let by_type = request(&mut table, aid, &state, ("03", ""), &[]);
            assert_eq!(codes(&by_type), texts(&["03", "", "", "", "", "", ""]));
            assert_eq!(
                (by_type.page_no(), by_type.message(), by_type.committed()),
                (PageNumber::FIRST, None, Committed::Nothing)
            );
            assert_eq!(
                (
                    by_type.type_filter().status(),
                    by_type.state().type_filter()
                ),
                (FilterStatus::Valid, "03")
            );
            assert_eq!(by_type.info(), Some(InfoMessage::RecordActions));
            let by_text = request(&mut table, aid, &state, ("", "R"), &[]);
            assert_eq!(
                codes(&by_text),
                texts(&["01", "03", "04", "05", "06", "", ""])
            );
            assert_eq!(
                (by_text.desc_filter().text(), by_text.committed()),
                ("R", Committed::Nothing)
            );
        }
        let case_sensitive = request(&mut table, ListAid::Enter, &state, ("", "  PAY  "), &[]);
        assert_eq!(
            codes(&case_sensitive),
            texts(&["02", "", "", "", "", "", ""])
        );
        assert_eq!(table.rows.len(), 7);
        Ok(())
    }

    // audit: COTRTLIC-019
    // audit: COTRTLIC-041
    // audit: COTRTLIC-042
    // audit: COTRTLIC-049
    #[test]
    fn a_delete_kept_across_a_changed_type_filter_is_committed_on_the_new_row() -> TestResult {
        let mut table = Table::seed()?;
        table.parents.retain(|parent| parent.to_text() != "05");
        let state = loaded(&mut table);
        // The `D` was typed on row 1 (type 01) of the old page; the selections are never cleared
        // (E28), so it stays at row 1, which now holds type 05.
        let delete = rows_with(&[(1, "D", "")]);
        let kept = request(&mut table, ListAid::Enter, &state, ("05", ""), &delete);
        assert_eq!(codes(&kept), texts(&["05", "", "", "", "", "", ""]));
        assert_eq!(
            (shown(&kept, 1)?.selection(), kept.state().pending()),
            (Selection::Delete, PendingAction::Delete)
        );
        assert_eq!((kept.info(), kept.message()), (None, None));
        let committed = request(&mut table, ListAid::Pf10, kept.state(), ("05", ""), &delete);
        assert_eq!(committed.committed(), Committed::Deleted);
        assert_eq!(
            committed.info().map(InfoMessage::text),
            Some("HIGHLIGHTED row deleted.Hit Enter to continue")
        );
        assert!(
            table.find(&PicX::from_text("05")?).is_none(),
            "COTRTLIC-042"
        );
        assert!(
            table.find(&PicX::from_text("01")?).is_some(),
            "COTRTLIC-042"
        );
        assert_eq!(
            (committed.state().pending(), committed.state().selected()),
            (PendingAction::None, None)
        );
        Ok(())
    }

    // audit: COTRTLIC-018
    // audit: COTRTLIC-040
    // audit: COTRTLIC-044
    // audit: COTRTLIC-045
    #[test]
    fn an_edited_description_moves_to_the_new_row_and_f10_updates_that_row() -> TestResult {
        let mut table = Table::seed()?;
        let state = loaded(&mut table);
        let opened = request(
            &mut table,
            ListAid::Enter,
            &state,
            ("", ""),
            &rows_with(&[(2, "U", "")]),
        );
        assert_eq!(
            opened.info().map(InfoMessage::text),
            Some("Update HIGHLIGHTED row. Press F10 to save")
        );
        assert_eq!(opened.state().description_entry(), RowIndex::from_number(2));
        let edited = rows_with(&[(2, "U", "RENAMED CREDIT")]);
        let moved = request(
            &mut table,
            ListAid::Enter,
            opened.state(),
            ("", "R"),
            &edited,
        );
        assert_eq!(
            codes(&moved),
            texts(&["01", "03", "04", "05", "06", "", ""])
        );
        assert_eq!(
            (shown(&moved, 2)?.description(), moved.state().pending()),
            ("RENAMED CREDIT", PendingAction::Update)
        );
        let saved = request(&mut table, ListAid::Pf10, moved.state(), ("", "R"), &edited);
        assert_eq!(
            (saved.committed(), saved.message()),
            (Committed::Updated, None)
        );
        assert_eq!(
            saved.info().map(InfoMessage::text),
            Some("HIGHLIGHTED row was updated")
        );
        assert_eq!(
            table.description("03")?,
            format!("{:<50}", "RENAMED CREDIT")
        );
        assert_eq!(table.description("02")?, "PAYMENT");
        assert_eq!(
            (shown(&saved, 2)?.selection(), saved.state().pending()),
            (Selection::Blank, PendingAction::None)
        );
        Ok(())
    }

    // audit: COTRTLIC-034
    // audit: COTRTLIC-038
    #[test]
    fn f10_with_a_pending_action_and_a_changed_filter_commits_nothing() -> TestResult {
        let mut table = Table::seed()?;
        table.parents.clear();
        let state = loaded(&mut table);
        let delete = rows_with(&[(3, "D", "")]);
        let pending = request(&mut table, ListAid::Enter, &state, ("", ""), &delete);
        assert_eq!(pending.state().pending(), PendingAction::Delete);
        let refiltered = request(
            &mut table,
            ListAid::Pf10,
            pending.state(),
            ("", "R"),
            &delete,
        );
        assert_eq!(
            (refiltered.committed(), table.rows.len()),
            (Committed::Nothing, 7)
        );
        assert_eq!(
            codes(&refiltered),
            texts(&["01", "03", "04", "05", "06", "", ""])
        );
        assert_eq!(
            (
                shown(&refiltered, 3)?.selection(),
                refiltered.state().pending()
            ),
            (Selection::Delete, PendingAction::Delete)
        );
        let invalid = request(
            &mut table,
            ListAid::Pf10,
            pending.state(),
            ("", "R"),
            &rows_with(&[(2, "X", "")]),
        );
        assert_eq!(
            invalid.message_text().as_deref(),
            Some("Action code selected is invalid")
        );
        assert_eq!(
            (invalid.committed(), invalid.status()),
            (Committed::Nothing, StatusClass::Input)
        );
        assert_eq!(
            codes(&invalid),
            texts(&["01", "03", "04", "05", "06", "", ""])
        );
        Ok(())
    }

    // audit: COTRTLIC-020
    // audit: COTRTLIC-021
    // audit: COTRTLIC-022
    // audit: COTRTLIC-023
    #[test]
    fn invalid_codes_and_two_selections_are_input_errors() -> TestResult {
        let mut table = Table::seed()?;
        let state = loaded(&mut table);
        for code in ["d", "u", "X", "*"] {
            let outcome = request(
                &mut table,
                ListAid::Enter,
                &state,
                ("", ""),
                &rows_with(&[(1, code, "")]),
            );
            assert_eq!(
                outcome.message_text().as_deref(),
                Some("Action code selected is invalid"),
                "{code}"
            );
            assert_eq!(
                (outcome.status().http_status(), outcome.state().pending()),
                (400, PendingAction::None)
            );
        }
        let two = request(
            &mut table,
            ListAid::Enter,
            &state,
            ("", ""),
            &rows_with(&[(1, "D", ""), (3, "U", "")]),
        );
        assert_eq!(
            two.message_text().as_deref(),
            Some("Please select only 1 action")
        );
        assert_eq!(
            (two.status(), two.state().pending()),
            (StatusClass::Input, PendingAction::None)
        );
        assert_eq!(two.state().selected(), RowIndex::from_number(1));
        assert_eq!(two.info(), Some(InfoMessage::RecordActions));
        Ok(())
    }

    // audit: COTRTLIC-024
    // audit: COTRTLIC-025
    // audit: COTRTLIC-026
    #[test]
    fn the_update_description_is_compared_then_edited() -> TestResult {
        let mut table = Table::seed()?;
        let state = loaded(&mut table);
        let first = request(
            &mut table,
            ListAid::Enter,
            &state,
            ("", ""),
            &rows_with(&[(1, "U", "SOMETHING ELSE")]),
        );
        assert_eq!(
            first.message_text().as_deref(),
            Some("No change detected with respect to database values.")
        );
        assert_eq!(
            (first.status(), first.state().pending()),
            (StatusClass::Ok, PendingAction::Update)
        );
        assert_eq!(shown(&first, 1)?.description(), "PURCHASE");
        for blank in ["", "   ", "*"] {
            let missing = request(
                &mut table,
                ListAid::Enter,
                first.state(),
                ("", ""),
                &rows_with(&[(1, "U", blank)]),
            );
            assert_eq!(
                missing.message_text().as_deref(),
                Some("Transaction Desc must be supplied."),
                "{blank:?}"
            );
            assert_eq!(
                (shown(&missing, 1)?.description(), missing.status()),
                ("*", StatusClass::Input)
            );
        }
        let symbol = request(
            &mut table,
            ListAid::Enter,
            first.state(),
            ("", ""),
            &rows_with(&[(1, "U", "BAD;DESC")]),
        );
        assert_eq!(
            symbol.message_text().as_deref(),
            Some("Transaction Desc can have numbers or alphabets only.")
        );
        let refused = request(
            &mut table,
            ListAid::Pf10,
            first.state(),
            ("", ""),
            &rows_with(&[(1, "U", "BAD;DESC")]),
        );
        assert_eq!(
            (refused.committed(), table.description("01")?),
            (Committed::Nothing, "PURCHASE".to_owned())
        );
        let saved = request(
            &mut table,
            ListAid::Pf10,
            first.state(),
            ("", ""),
            &rows_with(&[(1, "U", "purchase")]),
        );
        assert_eq!(
            (saved.committed(), saved.message()),
            (Committed::Updated, Some(ListMessage::NoChangeDetected))
        );
        assert_eq!(table.description("01")?, format!("{:<50}", "purchase"));
        Ok(())
    }

    // audit: COTRTLIC-039
    // audit: COTRTLIC-043
    // audit: COTRTLIC-050
    #[test]
    fn a_delete_is_prompted_then_committed_unless_categories_refer_to_it() -> TestResult {
        let mut table = Table::seed()?;
        let state = loaded(&mut table);
        let last = rows_with(&[(7, "D", "")]);
        let prompt = request(&mut table, ListAid::Enter, &state, ("", ""), &last);
        assert_eq!(
            prompt.info().map(InfoMessage::text),
            Some("Delete HIGHLIGHTED row ? Press F10 to confirm")
        );
        assert_eq!(
            (prompt.state().pending(), table.rows.len()),
            (PendingAction::Delete, 7)
        );
        let deleted = request(&mut table, ListAid::Pf10, prompt.state(), ("", ""), &last);
        assert_eq!(deleted.committed(), Committed::Deleted);
        assert!(
            table.find(&PicX::from_text("07")?).is_none(),
            "COTRTLIC-049"
        );
        let first = rows_with(&[(1, "D", "")]);
        let reloaded = loaded(&mut table);
        let pending = request(&mut table, ListAid::Enter, &reloaded, ("", ""), &first);
        let refused = request(&mut table, ListAid::Pf10, pending.state(), ("", ""), &first);
        assert!(
            refused
                .message_text()
                .unwrap_or_default()
                .starts_with("Please delete associated child records first: SQLCODE:"),
            "COTRTLIC-050"
        );
        assert_eq!(
            (refused.status().http_status(), refused.committed()),
            (400, Committed::Nothing)
        );
        assert_eq!(
            (refused.state().pending(), table.rows.len()),
            (PendingAction::Delete, 6)
        );
        Ok(())
    }

    // audit: COTRTLIC-027
    // audit: COTRTLIC-028
    // audit: COTRTLIC-031
    #[test]
    fn the_type_filter_is_two_digits_and_a_filter_must_match_rows() -> TestResult {
        let mut table = Table::seed()?;
        let state = loaded(&mut table);
        let zeros = request(&mut table, ListAid::Enter, &state, ("00", ""), &[]);
        assert_eq!(
            codes(&zeros),
            texts(&["01", "02", "03", "04", "05", "06", "07"])
        );
        for bad in ["1", "A1", "1 "] {
            let outcome = request(&mut table, ListAid::Enter, &state, (bad, ""), &[]);
            assert_eq!(
                outcome.message_text().as_deref(),
                Some("TYPE CODE FILTER,IF SUPPLIED MUST BE A 2 DIGIT NUMBER"),
                "{bad:?}"
            );
            assert_eq!(outcome.status().http_status(), 400);
        }
        let none = request(&mut table, ListAid::Enter, &state, ("09", ""), &[]);
        assert_eq!(
            none.message_text().as_deref(),
            Some("No Records found for these filter conditions")
        );
        assert_eq!(
            (none.status().http_status(), none.committed()),
            (400, Committed::Nothing)
        );
        Ok(())
    }

    /// A failing commit or read: the Db2 texts after ` SQLCODE:` are HALTED(H3), so these run
    /// for coverage only and no value is asserted.
    #[test]
    fn store_failures_run_without_parity_assertions() -> TestResult {
        let mut table = Table::seed()?;
        let state = loaded(&mut table);
        let pending = request(
            &mut table,
            ListAid::Enter,
            &state,
            ("", ""),
            &rows_with(&[(7, "D", "")]),
        );
        table.rows.clear();
        let _ = request(
            &mut table,
            ListAid::Pf10,
            pending.state(),
            ("", ""),
            &rows_with(&[(7, "D", "")]),
        );
        Ok(())
    }
}

mod csutldtc {
    use domain::online::csutldtc::{
        DateMask, LS_RESULT_LEN, LeFeedback, LeResult, Part, caller_accepts, edit_date_le, validate,
    };

    use super::TestResult;

    /// The feedback token `CEEDAYS` returns for condition `CEE` severity 3, message `msg_no`.
    const fn token(msg_no: u8) -> [u8; 8] {
        [0x00, 0x03, 0x09, msg_no, 0x59, 0xC3, 0xC5, 0xC5]
    }

    // audit: CSUTLDTC-003
    // audit: CSUTLDTC-008
    #[test]
    fn the_result_record_lays_out_codes_text_and_mask_around_the_halted_date() {
        let record =
            LeResult::new("2020-01-31", DateMask::YyyyMmDdDash, LeFeedback::Valid).to_record();
        assert_eq!((record.len(), LS_RESULT_LEN), (80, 80));
        assert_eq!(
            record.get(..45),
            Some(&b"0000Mesg Code: 0000 Date is valid   TstDate: "[..])
        );
        // Positions 46-55 are HALTED(H5): the group move after CEEDAYS copies whatever CEEDAYS
        // leaves in its input string, so they are reported BLOCKED and not asserted.
        assert_eq!(record.get(55..), Some(&b" Mask used:YYYY-MM-DD    "[..]));
        let rejected = LeResult::new(
            "2020-02-30",
            DateMask::YyyyMmDdDash,
            LeFeedback::BadDateValue,
        );
        assert_eq!(
            (rejected.severity_code(), rejected.msg_no()),
            ("0003", "2508")
        );
        assert_eq!(
            (DateMask::YyyyMmDdDash.text(), DateMask::YyyyMmDd.text()),
            ("YYYY-MM-DD", "YYYYMMDD")
        );
    }

    // audit: CSUTLDTC-009
    // audit: CSUTLDTC-010
    // audit: CSUTLDTC-011
    // audit: CSUTLDTC-012
    // audit: CSUTLDTC-013
    // audit: CSUTLDTC-014
    // audit: CSUTLDTC-015
    // audit: CSUTLDTC-016
    // audit: CSUTLDTC-017
    // audit: CSUTLDTC-018
    #[test]
    fn each_feedback_token_names_its_result_text() {
        let cases = [
            ([0; 8], LeFeedback::Valid, "Date is valid  ", (0, 0)),
            (
                token(0xCB),
                LeFeedback::InsufficientData,
                "Insufficient   ",
                (3, 2507),
            ),
            (
                token(0xCC),
                LeFeedback::BadDateValue,
                "Datevalue error",
                (3, 2508),
            ),
            (
                token(0xCD),
                LeFeedback::InvalidEra,
                "Invalid Era    ",
                (3, 2509),
            ),
            (
                token(0xD1),
                LeFeedback::UnsupportedRange,
                "Unsupp. Range  ",
                (3, 2513),
            ),
            (
                token(0xD5),
                LeFeedback::InvalidMonth,
                "Invalid month  ",
                (3, 2517),
            ),
            (
                token(0xD6),
                LeFeedback::BadPicString,
                "Bad Pic String ",
                (3, 2518),
            ),
            (
                token(0xD8),
                LeFeedback::NonNumericData,
                "Nonnumeric data",
                (3, 2520),
            ),
            (
                token(0xD9),
                LeFeedback::YearInEraZero,
                "YearInEra is 0 ",
                (3, 2521),
            ),
        ];
        for (bytes, feedback, text, (severity, msg_no)) in cases {
            assert_eq!(LeFeedback::from_token(bytes), feedback, "{text}");
            assert_eq!(
                (
                    feedback.result_text(),
                    feedback.severity(),
                    feedback.msg_no()
                ),
                (text, severity, msg_no)
            );
        }
        let other = LeFeedback::from_token([0x00, 0x00, 0x00, 0x07, 0, 0, 0, 0]);
        assert_eq!(
            other,
            LeFeedback::Other {
                severity: 0,
                msg_no: 7
            }
        );
        assert_eq!(other.result_text(), "Date is invalid");
        assert_eq!(
            LeFeedback::from_token([0x00, 0x03, 0x09, 0xCC, 0x59, 0xC3, 0xC5, 0x00]).result_text(),
            "Date is invalid"
        );
    }

    // audit: CSUTLDTC-022
    // audit: CSUTLDTC-023
    // audit: CSUTLDTC-024
    // audit: CSUTLDTC-029
    // audit: CSUTLDTC-031
    #[test]
    fn the_callers_judge_the_verdict_by_severity_and_message_number() -> TestResult {
        let accepted = LeResult::new("20200131", DateMask::YyyyMmDd, LeFeedback::Valid);
        assert_eq!(edit_date_le("Date of Birth", &accepted), Ok(()));
        let rejected = LeResult::new("20200230", DateMask::YyyyMmDd, LeFeedback::BadDateValue);
        let failure = edit_date_le("Date of Birth", &rejected)
            .err()
            .ok_or("a severity 3 verdict passed")?;
        assert_eq!(
            failure.message(),
            "Date of Birth validation error Sev code: 0003 Message code: 2508"
        );
        assert_eq!(failure.part(), Part::Year);
        for (feedback, accepts) in [
            (LeFeedback::Valid, true),
            (LeFeedback::UnsupportedRange, true),
            (
                LeFeedback::Other {
                    severity: 3,
                    msg_no: 2513,
                },
                true,
            ),
            (
                LeFeedback::Other {
                    severity: 0,
                    msg_no: 7,
                },
                true,
            ),
            (LeFeedback::BadDateValue, false),
            (LeFeedback::NonNumericData, false),
            (
                LeFeedback::Other {
                    severity: 4,
                    msg_no: 2512,
                },
                false,
            ),
        ] {
            let result = LeResult::new("2022-07-18", DateMask::YyyyMmDdDash, feedback);
            assert_eq!(caller_accepts(&result), accepts, "{feedback:?}");
        }
        Ok(())
    }

    /// HALTED(H5): which dates `CEEDAYS` accepts is Language Environment's; `validate` runs here
    /// for coverage only and no verdict is asserted.
    #[test]
    fn validity_runs_without_parity_assertions() {
        for (text, mask) in [
            ("2022-07-18", DateMask::YyyyMmDdDash),
            ("2022-02-30", DateMask::YyyyMmDdDash),
            ("20220718", DateMask::YyyyMmDd),
            ("ABCDEFGH", DateMask::YyyyMmDd),
        ] {
            let _ = validate(text, mask);
        }
    }
}
