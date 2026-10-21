//! The rule rows of the three MQ-triggered programs whose outcome a pure `domain::messaging`
//! function decides: CDRD (`CODATE01`), CDRA (`COACCT01`) and CP00 (`COPAUA0C`), as
//! `modern/audit/messaging/{CODATE01,COACCT01,COPAUA0C}.md` and the CP00 rows of
//! `modern/audit/arithmetic.md` state them. This is the domain half of binding g3a, and it holds
//! invariants i10 (the credit-limit guard, E30) and i11 (the total request parse, E31) at the
//! decision level, in one process.
//!
//! Broker properties, routing, confirmation, acknowledgement, dead-lettering, the summary row
//! lock, concurrency and redelivery belong to `modern/crates/messaging/tests/*.rs`. Nothing here
//! opens a broker, a database, a network connection or an environment variable: the account,
//! cross-reference and customer fixtures are read in place from `app/data/ASCII`, and every time
//! comes from a `FixedClock` or a test clock.
//!
//! # Halts
//!
//! A row the audit marks `HALTED(Hn)`, or blocks through a halt candidate, still carries its
//! `// audit:` tag, because `modern/audit/README.md` ("Layers and required tags") and
//! `audit_coverage.rs` (a8) require every row to be tagged. Its test runs the path for coverage,
//! prints `BLOCKED <rule> (<halt>)` and asserts no parity value (AAP 0.8.5); where the AAP itself
//! fixes the replacement's behaviour, that behaviour is asserted and labelled as such. The CP00
//! reply's 63 written bytes are asserted, because `HALTS.md` (H6) blocks only the body's length
//! and its 64th byte (COPAUA0C-060 to COPAUA0C-065).
//!
//! # Redaction
//!
//! No assertion message carries a request body, a card number or an SSN: messages name a rule ID,
//! and CP00 card fields are compared through their redacted form or without being printed
//! (Cardholder Data Redaction).

use std::error::Error;
use std::fs;
use std::path::Path;

use chrono::{NaiveDate, NaiveDateTime};
use copybook::{FixedRecord, lines};
use domain::batch::loads::decode_fixture;
use domain::messaging::codate01::{BUFFER_LEN, MQ_ERR_DISPLAY_LEN};
use domain::money::{Decimal, Money};

/// The result of a test whose setup can fail.
type TestResult = Result<(), Box<dyn Error>>;

/// The bytes of fixture `name` in `app/data/ASCII`, read in place.
fn fixture(name: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../app/data/ASCII")
        .join(name);
    Ok(fs::read(path)?)
}

/// Every record of fixture `name`, decoded with codec A in file order.
fn fixture_records<R: FixedRecord>(name: &str) -> Result<Vec<R>, Box<dyn Error>> {
    Ok(decode_fixture::<R>(&fixture(name)?)?
        .into_iter()
        .map(|framed| framed.record)
        .collect())
}

/// The body of every line of fixture `name`, without its terminator, in file order.
fn fixture_lines(name: &str) -> Result<Vec<Vec<u8>>, Box<dyn Error>> {
    Ok(lines(&fixture(name)?)
        .map(|line| line.body.to_vec())
        .collect())
}

/// The `len` bytes of `bytes` from `start`.
fn segment(bytes: &[u8], start: usize, len: usize) -> Result<&[u8], Box<dyn Error>> {
    start
        .checked_add(len)
        .and_then(|end| bytes.get(start..end))
        .ok_or_else(|| Box::<dyn Error>::from(format!("bytes {start} to {start}+{len} absent")))
}

/// Whether every byte is a space.
fn all_spaces(bytes: &[u8]) -> bool {
    bytes.iter().all(|&byte| byte == b' ')
}

/// `text` followed by spaces to `width` bytes.
fn padded(text: &[u8], width: usize) -> Vec<u8> {
    let mut field = text.to_vec();
    field.resize(width, b' ');
    field
}

/// Whether `bytes` has the layout `pattern`, where `9` stands for any ASCII digit and every other
/// byte for itself.
fn has_format(bytes: &[u8], pattern: &[u8]) -> bool {
    bytes.len() == pattern.len()
        && bytes.iter().zip(pattern).all(|(&byte, &shape)| {
            if shape == b'9' {
                byte.is_ascii_digit()
            } else {
                byte == shape
            }
        })
}

/// Whether `needle`, which is not empty, occurs in `haystack`.
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

/// The instant `date` (year, month, day) at `time` (hour, minute, second, millisecond).
fn instant(
    date: (i32, u32, u32),
    time: (u32, u32, u32, u32),
) -> Result<NaiveDateTime, Box<dyn Error>> {
    let (year, month, day) = date;
    let (hour, minute, second, milli) = time;
    NaiveDate::from_ymd_opt(year, month, day)
        .and_then(|day| day.and_hms_milli_opt(hour, minute, second, milli))
        .ok_or_else(|| Box::<dyn Error>::from("no such instant"))
}

/// `cents` hundredths as an amount of picture `S9(N)V99`.
fn money<const N: u32>(cents: i64) -> Result<Money<N>, Box<dyn Error>> {
    Ok(Money::new(Decimal::new(cents, 2))?)
}

/// Reports a case a halt or a halt candidate blocks; its test asserts no parity value for it
/// (AAP 0.8.5).
fn blocked(rule: &str, halt: &str) {
    println!("BLOCKED {rule} ({halt})");
}

/// Asserts every byte of a 1,000-byte `MQ-ERR-DISPLAY` error body outside its two code fields,
/// which halt H4 leaves unasserted (bytes 54-55 and 58-62): `MQ-ERROR-PARA` and the four
/// `FILLER`s spaces, `message` and `queue` space-padded to 25 and 48 bytes, then 887 spaces
/// (CODATE01-015, COACCT01-029).
fn assert_error_body(body: &[u8], message: &[u8], queue: &[u8], rule: &str) -> TestResult {
    assert_eq!(body.len(), BUFFER_LEN, "{rule}");
    let expected: [(usize, Vec<u8>); 8] = [
        (0, padded(b"", 25)),
        (25, padded(b"", 2)),
        (27, padded(message, 25)),
        (52, padded(b"", 2)),
        (56, padded(b"", 2)),
        (63, padded(b"", 2)),
        (65, padded(queue, 48)),
        (
            MQ_ERR_DISPLAY_LEN,
            padded(b"", BUFFER_LEN - MQ_ERR_DISPLAY_LEN),
        ),
    ];
    for (offset, bytes) in &expected {
        assert_eq!(
            segment(body, *offset, bytes.len())?,
            bytes.as_slice(),
            "{rule}"
        );
    }
    Ok(())
}

/// CDRD: the reply and error bodies `CODATE01` builds (`modern/audit/messaging/CODATE01.md`).
mod cdrd {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use chrono::NaiveDateTime;
    use copybook::PicU;
    use domain::clock::{Clock, FixedClock};
    use domain::messaging::codate01::{
        BUFFER_LEN, ERROR_QUEUE, MQ_ERR_DISPLAY_LEN, MqErrDisplay, REPLY_QUEUE, REPLY_TEXT_LEN,
        ReturnMessage, reply_body, space_padded,
    };

    use super::{TestResult, all_spaces, assert_error_body, blocked, has_format, instant, segment};

    /// A clock that reads `first` once and `later` on every reading after it, counting readings.
    struct TwoInstantClock {
        first: NaiveDateTime,
        later: NaiveDateTime,
        reads: AtomicUsize,
    }

    impl Clock for TwoInstantClock {
        fn now(&self) -> NaiveDateTime {
            if self.reads.fetch_add(1, Ordering::SeqCst) == 0 {
                self.first
            } else {
                self.later
            }
        }
    }

    // audit: CODATE01-003
    // audit: CODATE01-004
    // audit: CODATE01-005
    // audit: CODATE01-006
    // audit: CODATE01-007
    #[test]
    fn the_reply_is_the_date_and_time_text_then_spaces() -> TestResult {
        // Midnight on a leap day, the last millisecond of a year and an afternoon: every reading
        // renders as `MM-DD-YYYY` and `HH:MM:SS`, the fraction of a second dropped. The values
        // are compared by format only.
        let instants = [
            instant((2024, 2, 29), (0, 0, 0, 0))?,
            instant((1999, 12, 31), (23, 59, 59, 999))?,
            instant((2022, 7, 18), (13, 45, 7, 123))?,
        ];
        for at in instants {
            let body = reply_body(&FixedClock::new(at));
            assert_eq!(body.len(), BUFFER_LEN, "CODATE01-007");
            assert_eq!(segment(&body, 0, 14)?, b"SYSTEM DATE : ", "CODATE01-003");
            assert!(
                has_format(segment(&body, 14, 10)?, b"99-99-9999"),
                "CODATE01-004"
            );
            assert_eq!(segment(&body, 24, 14)?, b"SYSTEM TIME : ", "CODATE01-005");
            assert!(
                has_format(segment(&body, 38, 8)?, b"99:99:99"),
                "CODATE01-006"
            );
            assert!(
                all_spaces(segment(&body, REPLY_TEXT_LEN, BUFFER_LEN - REPLY_TEXT_LEN)?),
                "CODATE01-007"
            );
        }
        assert_eq!(REPLY_TEXT_LEN, 46, "CODATE01-007");
        assert_eq!(BUFFER_LEN, 1_000, "CODATE01-007");
        Ok(())
    }

    // audit: CODATE01-008
    #[test]
    fn the_date_and_the_time_come_from_one_clock_reading() -> TestResult {
        let first = instant((1999, 12, 31), (23, 59, 59, 999))?;
        let later = instant((2000, 1, 1), (0, 0, 0, 0))?;
        let clock = TwoInstantClock {
            first,
            later,
            reads: AtomicUsize::new(0),
        };
        let body = reply_body(&clock);
        assert_eq!(clock.reads.load(Ordering::SeqCst), 1, "CODATE01-008");
        // A second reading would cross midnight, so a body built from two readings could pair the
        // old date with the new time. The body is the one built from the first reading alone.
        assert!(body == reply_body(&FixedClock::new(first)), "CODATE01-008");
        assert!(body != reply_body(&FixedClock::new(later)), "CODATE01-008");
        Ok(())
    }

    // audit: CODATE01-013
    #[test]
    fn each_reply_depends_only_on_its_own_reading() -> TestResult {
        let morning = FixedClock::new(instant((2022, 7, 18), (8, 0, 1, 0))?);
        let evening = FixedClock::new(instant((2023, 1, 2), (21, 30, 0, 500))?);
        let first = reply_body(&morning);
        let second = reply_body(&evening);
        let third = reply_body(&morning);
        assert!(first == third, "CODATE01-013");
        assert!(first != second, "CODATE01-013");
        Ok(())
    }

    // audit: CODATE01-007
    // audit: CODATE01-015
    #[test]
    fn a_message_area_is_its_content_then_spaces_cut_at_the_buffer() -> TestResult {
        let short = space_padded(b"ABC");
        assert_eq!(segment(&short, 0, 3)?, b"ABC", "CODATE01-007");
        assert!(
            all_spaces(segment(&short, 3, BUFFER_LEN - 3)?),
            "CODATE01-007"
        );
        let long: Vec<u8> = (0..1_200_u32)
            .map(|n| if n % 2 == 0 { b'X' } else { b'Y' })
            .collect();
        let cut = space_padded(&long);
        assert_eq!(
            cut.as_slice(),
            segment(&long, 0, BUFFER_LEN)?,
            "CODATE01-015"
        );
        assert!(all_spaces(&space_padded(b"")), "CODATE01-015");
        Ok(())
    }

    // audit: CODATE01-014
    // audit: CODATE01-015
    #[test]
    fn a_failed_reply_put_gives_the_mq_err_display_body() -> TestResult {
        assert_eq!(REPLY_QUEUE, "CARD.DEMO.REPLY.DATE", "CODATE01-014");
        assert_eq!(ERROR_QUEUE, "CARD.DEMO.ERROR", "CODATE01-014");
        assert_eq!(
            ReturnMessage::MqPutErr.literal(),
            "MQPUT ERR",
            "CODATE01-014"
        );
        let message = MqErrDisplay::for_failure(ReturnMessage::MqPutErr, REPLY_QUEUE);
        assert!(message.error_para.is_spaces(), "CODATE01-015");
        let group = message.encode();
        assert_eq!(MQ_ERR_DISPLAY_LEN, 113, "CODATE01-015");
        assert_eq!(group.len(), MQ_ERR_DISPLAY_LEN, "CODATE01-015");
        let body = message.error_body();
        assert_eq!(
            segment(&body, 0, MQ_ERR_DISPLAY_LEN)?,
            group.as_slice(),
            "CODATE01-015"
        );
        assert_error_body(&body, b"MQPUT ERR", b"CARD.DEMO.REPLY.DATE", "CODATE01-015")?;
        // Whatever the code fields hold, no other byte moves.
        let coded = message
            .with_codes(PicU::new(2)?, PicU::new(2_085)?)
            .error_body();
        assert_error_body(
            &coded,
            b"MQPUT ERR",
            b"CARD.DEMO.REPLY.DATE",
            "CODATE01-015",
        )?;
        Ok(())
    }

    // audit: CODATE01-016
    // audit: CODATE01-017
    #[test]
    fn the_error_body_codes_are_blocked() -> TestResult {
        // HALTED(H4): the MQ completion and reason codes at bytes 54-55 and 58-62 have no AMQP
        // counterpart. The body is built with codes for coverage; their bytes are not asserted.
        let body = MqErrDisplay::for_failure(ReturnMessage::MqPutErr, REPLY_QUEUE)
            .with_codes(PicU::new(2)?, PicU::new(2_033)?)
            .error_body();
        assert_eq!(body.len(), BUFFER_LEN, "CODATE01-015");
        blocked("CODATE01-016", "H4");
        blocked("CODATE01-017", "H4");
        Ok(())
    }
}

/// CDRA: the request decoder, the branch and the reply and error bodies of `COACCT01`
/// (`modern/audit/messaging/COACCT01.md`).
mod cdra {
    use std::error::Error;

    use chrono::NaiveDate;
    use copybook::{PicDate, PicU, PicX};
    use domain::messaging::coacct01::{
        ACCT_RESPONSE_LEN, AccountLookup, BAD_PARAMETER_TEXT_LEN, CdraAction, CdraRequestError,
        CdraResponse, ERROR_QUEUE, NOT_FOUND_TEXT_LEN, REPLY_QUEUE, REQUEST_LEN, account_response,
        decide, decode_request, read_failure, respond, undecoded_reply,
    };
    use domain::messaging::codate01::{BUFFER_LEN, MqErrDisplay, ReturnMessage};
    use domain::money::{Decimal, Money};
    use domain::records::AccountRecord;

    use super::{
        TestResult, all_spaces, assert_error_body, blocked, contains, fixture_lines,
        fixture_records, money, padded, segment,
    };

    /// The configured request queue an error body names: the `.env.example` default of
    /// `CARDDEMO_QUEUE_REQUEST_ACCT`.
    const REQUEST_QUEUE: &str = "CARD.DEMO.REQUEST.ACCT";

    /// One label of `WS-ACCT-RESPONSE` and the account field after it.
    struct ResponseField {
        /// The label's `VALUE`.
        label: &'static [u8],
        /// The label's reply offset.
        label_at: usize,
        /// The field's reply offset.
        value_at: usize,
        /// The field's width.
        width: usize,
        /// The offset of the same field in a `CVACT01Y` fixture line.
        record_at: usize,
        /// The rule the pair proves.
        rule: &'static str,
    }

    /// `WS-ACCT-RESPONSE` in declaration order (COACCT01-006 to COACCT01-016).
    const RESPONSE: [ResponseField; 11] = [
        field(b"ACCOUNT ID : ", 0, 11, 0, "COACCT01-006"),
        field(b"ACCOUNT STATUS : ", 24, 1, 11, "COACCT01-007"),
        field(b"BALANCE : ", 42, 12, 12, "COACCT01-008"),
        field(b"CREDIT LIMIT : ", 64, 12, 24, "COACCT01-009"),
        field(b"CASH LIMIT : ", 91, 12, 36, "COACCT01-010"),
        field(b"OPEN DATE : ", 116, 10, 48, "COACCT01-011"),
        field(b"EXPR DATE : ", 138, 10, 58, "COACCT01-012"),
        field(b"REIS DATE : ", 160, 10, 68, "COACCT01-013"),
        field(b"CREDIT BAL : ", 182, 12, 78, "COACCT01-014"),
        field(b"DEBIT BAL : ", 207, 12, 90, "COACCT01-015"),
        field(b"GROUP ID : ", 231, 10, 112, "COACCT01-016"),
    ];

    /// A response field whose value follows its label directly.
    const fn field(
        label: &'static [u8],
        label_at: usize,
        width: usize,
        record_at: usize,
        rule: &'static str,
    ) -> ResponseField {
        ResponseField {
            label,
            label_at,
            value_at: label_at + label.len(),
            width,
            record_at,
            rule,
        }
    }

    /// The reply offsets of the five amounts' last bytes, which carry the sign overpunch
    /// (COACCT01-017).
    const AMOUNT_SIGN_BYTES: [usize; 5] = [63, 90, 115, 206, 230];

    /// A fixture account and the `CVACT01Y` bytes of the line it was decoded from.
    type FixtureAccount = (AccountRecord, Vec<u8>);

    /// The fixture accounts, in file order.
    fn accounts() -> Result<Vec<FixtureAccount>, Box<dyn Error>> {
        let records = fixture_records::<AccountRecord>("acctdata.txt")?;
        let raw = fixture_lines("acctdata.txt")?;
        assert_eq!(records.len(), raw.len(), "acctdata.txt");
        Ok(records.into_iter().zip(raw).collect())
    }

    /// The first fixture account, the base of the synthetic accounts.
    fn base_account() -> Result<AccountRecord, Box<dyn Error>> {
        let (account, _) = accounts()?.into_iter().next().ok_or("no fixture account")?;
        Ok(account)
    }

    /// A request body: `function`, `key`, then the `WS-FILLER` bytes `filler`.
    fn request_body(function: &[u8], key: &[u8], filler: &[u8]) -> Vec<u8> {
        [function, key, filler].concat()
    }

    /// The 1,000 bytes of a reply; an error body fails the test.
    fn reply_of(response: &CdraResponse) -> Result<[u8; BUFFER_LEN], Box<dyn Error>> {
        match response {
            CdraResponse::Reply(body) => Ok(*body),
            CdraResponse::Error(_) => Err("an error body, not a reply".into()),
        }
    }

    /// Asserts a not-found reply (`function` absent) or a bad-parameter reply: the literals,
    /// eleven `*` in the key positions, the function bytes after `FUNCTION : `, then spaces.
    fn assert_parameter_reply(body: &[u8], function: Option<&[u8]>, rule: &str) -> TestResult {
        assert_eq!(body.len(), BUFFER_LEN, "{rule}");
        assert_eq!(
            segment(body, 0, 27)?,
            b"INVALID REQUEST PARAMETERS ",
            "{rule}"
        );
        assert_eq!(segment(body, 27, 10)?, b"ACCT ID : ", "{rule}");
        // Replacement behaviour, not a parity result: Cardholder Data Redaction masks the key
        // the COBOL echoes (X-COACCT01-9).
        assert_eq!(segment(body, 37, 11)?, b"***********", "{rule}");
        let text_len = if let Some(function) = function {
            assert_eq!(segment(body, 48, 11)?, b"FUNCTION : ", "{rule}");
            assert_eq!(segment(body, 59, 4)?, function, "{rule}");
            BAD_PARAMETER_TEXT_LEN
        } else {
            NOT_FOUND_TEXT_LEN
        };
        assert!(
            all_spaces(segment(body, text_len, BUFFER_LEN - text_len)?),
            "{rule}"
        );
        Ok(())
    }

    // audit: COACCT01-001
    // audit: COACCT01-002
    #[test]
    fn the_request_is_four_function_bytes_and_eleven_key_digits() -> TestResult {
        assert_eq!(REQUEST_LEN, 15, "COACCT01-001");
        let fillers: [&[u8]; 3] = [b"", &[b'Z'; 985], &[0xFF; 1_200]];
        for (account, raw) in &accounts()? {
            let key = segment(raw, 0, 11)?;
            for filler in fillers {
                let request = decode_request(&request_body(b"INQA", key, filler))?;
                assert_eq!(request.function().as_bytes(), b"INQA", "COACCT01-002");
                assert!(request.key() == account.id, "COACCT01-002");
            }
        }
        // The function is taken exactly as received: never validated, trimmed or case-folded.
        let raw = decode_request(&request_body(
            &[0x00, 0xFF, b'q', b' '],
            b"99999999999",
            b"",
        ))?;
        assert_eq!(
            raw.function().as_bytes(),
            &[0x00, 0xFF, b'q', b' '],
            "COACCT01-002"
        );
        assert_eq!(raw.key().get(), 99_999_999_999, "COACCT01-002");
        Ok(())
    }

    // audit: COACCT01-002
    #[test]
    fn short_bodies_and_non_digit_keys_are_typed_errors() {
        for len in 0..REQUEST_LEN {
            assert_eq!(
                decode_request(&vec![b'A'; len]).err(),
                Some(CdraRequestError::ShortBody { len }),
                "COACCT01-002"
            );
        }
        let valid = request_body(b"INQA", b"00000000050", b"");
        for offset in 4..REQUEST_LEN {
            for bad in [b' ', b'{', b'}', b'A', b'-', 0xFF] {
                let mut body = valid.clone();
                if let Some(byte) = body.get_mut(offset) {
                    *byte = bad;
                }
                assert_eq!(
                    decode_request(&body).err(),
                    Some(CdraRequestError::NonDigitKey { offset }),
                    "COACCT01-002"
                );
            }
        }
    }

    // audit: COACCT01-003
    #[test]
    fn an_undecodable_body_gets_the_masked_interim_reply() -> TestResult {
        // HALTED(X-COACCT01-H1), HALTED(X-COACCT01-H2): a non-digit key and a short body have no
        // single COBOL result. The masked bad-parameter body is the replacement's interim
        // handling, checked as replacement behaviour and never reported as parity.
        let body = undecoded_reply();
        assert_parameter_reply(&body, Some(b"****"), "COACCT01-003")?;
        blocked("COACCT01-003", "X-COACCT01-H1");
        blocked("COACCT01-003", "X-COACCT01-H2");
        Ok(())
    }

    // audit: COACCT01-004
    #[test]
    fn inqa_with_a_non_zero_key_reads_that_account() -> TestResult {
        for (account, raw) in &accounts()? {
            let request = decode_request(&request_body(b"INQA", segment(raw, 0, 11)?, b""))?;
            assert!(
                decide(&request) == CdraAction::ReadAccount(account.id),
                "COACCT01-004"
            );
        }
        let highest = decode_request(&request_body(b"INQA", b"99999999999", b""))?;
        assert!(
            decide(&highest) == CdraAction::ReadAccount(PicU::new(99_999_999_999)?),
            "COACCT01-004"
        );
        Ok(())
    }

    // audit: COACCT01-005
    // audit: COACCT01-022
    #[test]
    fn other_functions_or_a_zero_key_get_the_bad_parameter_reply() -> TestResult {
        // The key `00` followed by nine digits, or `000` followed by eight, is masked like any
        // other (X-COACCT01-9).
        let cases: [(&[u8; 4], &[u8; 11]); 8] = [
            (b"inqa", b"00000000050"),
            (b"INQ ", b"00000000050"),
            (b"ACCT", b"00000000050"),
            (&[0x00, 0xFF, b'q', b' '], b"00000000050"),
            (b"INQA", b"00000000000"),
            (b"XXXX", b"00000000000"),
            (b"QQQQ", b"00123456789"),
            (b"QQQQ", b"00012345678"),
        ];
        for (function, key) in cases {
            let request = decode_request(&request_body(function, key, b""))?;
            let CdraAction::Reply(body) = decide(&request) else {
                return Err("COACCT01-005: the request read an account".into());
            };
            assert_parameter_reply(body.as_slice(), Some(function), "COACCT01-022")?;
            assert!(!contains(body.as_slice(), key), "COACCT01-022");
        }
        Ok(())
    }

    // audit: COACCT01-006
    // audit: COACCT01-007
    // audit: COACCT01-008
    // audit: COACCT01-009
    // audit: COACCT01-010
    // audit: COACCT01-011
    // audit: COACCT01-012
    // audit: COACCT01-013
    // audit: COACCT01-014
    // audit: COACCT01-015
    // audit: COACCT01-016
    // audit: COACCT01-020
    #[test]
    fn a_found_account_gets_the_account_response_then_spaces() -> TestResult {
        // The layout is contiguous and ends at 252 bytes.
        let mut next = 0;
        for field in &RESPONSE {
            assert_eq!(field.label_at, next, "{}", field.rule);
            next = field.value_at + field.width;
        }
        assert_eq!(next, ACCT_RESPONSE_LEN, "COACCT01-020");
        assert_eq!(ACCT_RESPONSE_LEN, 252, "COACCT01-020");
        // Every field equals the bytes of the fixture line the account was decoded from, built
        // from the record rather than from any quoted fixture value.
        let card_numbers: Vec<Vec<u8>> = fixture_lines("cardxref.txt")?
            .iter()
            .map(|line| segment(line, 0, 16).map(<[u8]>::to_vec))
            .collect::<Result<_, _>>()?;
        let accounts = accounts()?;
        assert_eq!(accounts.len(), 50, "acctdata.txt");
        for (account, raw) in &accounts {
            let body = reply_of(&respond(AccountLookup::Found(account), REQUEST_QUEUE))?;
            for field in &RESPONSE {
                assert_eq!(
                    segment(&body, field.label_at, field.label.len())?,
                    field.label,
                    "{}",
                    field.rule
                );
                assert_eq!(
                    segment(&body, field.value_at, field.width)?,
                    segment(raw, field.record_at, field.width)?,
                    "{}",
                    field.rule
                );
            }
            assert_eq!(
                segment(&body, 0, ACCT_RESPONSE_LEN)?,
                account_response(account).as_slice(),
                "COACCT01-020"
            );
            assert!(
                all_spaces(segment(
                    &body,
                    ACCT_RESPONSE_LEN,
                    BUFFER_LEN - ACCT_RESPONSE_LEN
                )?),
                "COACCT01-020"
            );
            // No CDRA body carries a card number (COACCT01-035, proved end to end by
            // `messaging/tests/redaction.rs`).
            assert!(
                card_numbers.iter().all(|card| !contains(&body, card)),
                "Cardholder Data Redaction"
            );
        }
        Ok(())
    }

    // audit: COACCT01-017
    #[test]
    fn amounts_are_zoned_display_with_a_trailing_overpunch() -> TestResult {
        let mut minus_zero = Decimal::new(0, 2);
        minus_zero.set_sign_negative(true);
        let account = AccountRecord {
            curr_bal: money(-12_345)?,
            credit_limit: money(101)?,
            cash_credit_limit: Money::new(minus_zero)?,
            curr_cyc_credit: money(-9)?,
            curr_cyc_debit: money(999_999_999_999)?,
            ..base_account()?
        };
        let response = account_response(&account);
        let expected: [(usize, &[u8; 12]); 5] = [
            (52, b"00000001234N"),
            (79, b"00000000010A"),
            (104, b"00000000000{"),
            (195, b"00000000000R"),
            (219, b"99999999999I"),
        ];
        for (offset, amount) in expected {
            assert_eq!(segment(&response, offset, 12)?, amount, "COACCT01-017");
        }
        // Every fixture amount is zero or positive, so its sign byte is `{` or `A` to `I`.
        for (account, _) in &accounts()? {
            let response = account_response(account);
            for offset in AMOUNT_SIGN_BYTES {
                let sign = segment(&response, offset, 1)?;
                assert!(matches!(sign, [b'{' | b'A'..=b'I']), "COACCT01-017");
            }
        }
        Ok(())
    }

    // audit: COACCT01-018
    #[test]
    fn dates_render_as_iso_text_or_ten_spaces() -> TestResult {
        let date = |year, month, day| -> Result<PicDate, Box<dyn Error>> {
            let value = NaiveDate::from_ymd_opt(year, month, day).ok_or("no such date")?;
            Ok(PicDate::new(Some(value))?)
        };
        let account = AccountRecord {
            open_date: date(2001, 2, 3)?,
            expiraion_date: date(2099, 12, 31)?,
            reissue_date: PicDate::BLANK,
            ..base_account()?
        };
        let response = account_response(&account);
        assert_eq!(segment(&response, 128, 10)?, b"2001-02-03", "COACCT01-018");
        assert_eq!(segment(&response, 150, 10)?, b"2099-12-31", "COACCT01-018");
        assert!(all_spaces(segment(&response, 172, 10)?), "COACCT01-018");
        Ok(())
    }

    // audit: COACCT01-019
    #[test]
    fn the_reply_carries_neither_the_zip_nor_the_filler() -> TestResult {
        let account = AccountRecord {
            addr_zip: PicX::from_text("ZIPZIPZIP1")?,
            group_id: PicX::from_text("GRP01")?,
            filler: PicX::from_array([b'#'; 178]),
            ..base_account()?
        };
        let body = reply_of(&respond(AccountLookup::Found(&account), REQUEST_QUEUE))?;
        assert!(!contains(&body, b"ZIPZIP"), "COACCT01-019");
        assert!(!body.contains(&b'#'), "COACCT01-019");
        assert_eq!(
            segment(&body, 231, 21)?,
            padded(b"GROUP ID : GRP01", 21).as_slice(),
            "COACCT01-019"
        );
        Ok(())
    }

    // audit: COACCT01-021
    #[test]
    fn a_missing_account_gets_the_not_found_reply() -> TestResult {
        assert_eq!(NOT_FOUND_TEXT_LEN, 48, "COACCT01-021");
        let body = reply_of(&respond(AccountLookup::NotFound, REQUEST_QUEUE))?;
        assert_parameter_reply(&body, None, "COACCT01-021")
    }

    // audit: COACCT01-027
    #[test]
    fn bodies_built_in_sequence_share_nothing() -> TestResult {
        let accounts = accounts()?;
        let (first, _) = accounts.first().ok_or("no fixture account")?;
        let (last, _) = accounts.last().ok_or("no fixture account")?;
        let bad_function = decode_request(&request_body(b"ACCT", b"00000000001", b""))?;
        // In flow order, then each again on its own: every body is built only from its own
        // request and read.
        let sequence = [
            respond(AccountLookup::Found(first), REQUEST_QUEUE),
            respond(AccountLookup::NotFound, REQUEST_QUEUE),
            respond(AccountLookup::Found(last), REQUEST_QUEUE),
            respond(AccountLookup::ReadFailed, REQUEST_QUEUE),
        ];
        let rejected = decide(&bad_function);
        let alone = [
            respond(AccountLookup::Found(first), REQUEST_QUEUE),
            respond(AccountLookup::NotFound, REQUEST_QUEUE),
            respond(AccountLookup::Found(last), REQUEST_QUEUE),
            respond(AccountLookup::ReadFailed, REQUEST_QUEUE),
        ];
        assert!(sequence == alone, "COACCT01-027");
        assert!(decide(&bad_function) == rejected, "COACCT01-027");
        assert!(sequence.first() != sequence.get(2), "COACCT01-027");
        Ok(())
    }

    // audit: COACCT01-028
    // audit: COACCT01-029
    #[test]
    fn a_failed_read_gets_the_error_body_and_no_reply() -> TestResult {
        assert_eq!(REPLY_QUEUE, "CARD.DEMO.REPLY.ACCT", "COACCT01-029");
        assert_eq!(ERROR_QUEUE, "CARD.DEMO.ERROR", "COACCT01-029");
        let CdraResponse::Error(body) = respond(AccountLookup::ReadFailed, REQUEST_QUEUE) else {
            return Err("COACCT01-028: a failed read produced a reply".into());
        };
        // `ERROR WHILE READING ACCTFILE` cut to its first 25 bytes (E18), and the request queue.
        assert_error_body(
            &body,
            b"ERROR WHILE READING ACCTF",
            REQUEST_QUEUE.as_bytes(),
            "COACCT01-029",
        )?;
        assert!(
            read_failure(REQUEST_QUEUE).error_body() == body,
            "COACCT01-029"
        );
        // A failed reply publication names the reply queue (the body of COACCT01-033).
        let put_failure = MqErrDisplay::for_failure(ReturnMessage::MqPutErr, REPLY_QUEUE);
        assert_error_body(
            &put_failure.error_body(),
            b"MQPUT ERR",
            b"CARD.DEMO.REPLY.ACCT",
            "COACCT01-029",
        )
    }

    // audit: COACCT01-030
    // audit: COACCT01-031
    #[test]
    fn the_read_failure_codes_are_blocked() -> TestResult {
        // HALTED(H4): the CICS `RESP` and `RESP2` of the failed read have no `PostgreSQL`
        // counterpart. The body is built with codes for coverage; their bytes are not asserted.
        let body = read_failure(REQUEST_QUEUE)
            .with_codes(PicU::new(13)?, PicU::new(80)?)
            .error_body();
        assert_eq!(body.len(), BUFFER_LEN, "COACCT01-029");
        blocked("COACCT01-030", "H4");
        blocked("COACCT01-031", "H4");
        Ok(())
    }
}

/// CP00: the request parse, the decision with the i10 guard, the reply bytes and the summary,
/// detail and error-log records of `COPAUA0C` (`modern/audit/messaging/COPAUA0C.md` and the
/// COPAUA0C rows of `modern/audit/arithmetic.md`).
mod cp00 {
    use std::error::Error;

    use chrono::NaiveDate;
    use copybook::edited::EditedPicture;
    use copybook::{Count, FixedRecord, PicS, PicU, PicX};
    use domain::clock::FixedClock;
    use domain::messaging::copaua0c::{
        ALERT_CARD_UNPARSEABLE, AuthStatus, CardNumberCause, CustomerLookup, Decision, FieldError,
        FieldFault, FoundContext, GET_BUFFER_LEN, GuardOutcome, LogSite, Lookup, PROGRAM,
        ParseError, REPLY_WRITTEN_LEN, ReasonCode, RequestField, SqlState, SummaryError,
        SummaryState, TRANSACTION, amount_screen, decide, detail_record, error_log,
        field_error_reply, parse_request, reply_body, summary_template, summary_update,
        transaction_id_screen,
    };
    use domain::money::{Decimal, Money, MoneyExpr};
    use domain::records::{
        AccountRecord, AuthReply, AuthRequestRecord, CardXrefRecord, CustomerRecord, ErrorLevel,
        ErrorLogRecord, ErrorSubsystem, PendingAuthDetailRecord, PendingAuthSummaryRecord,
    };
    use domain::sensitive::CardNumber;

    use super::{
        TestResult, blocked, contains, fixture_lines, fixture_records, has_format, instant, money,
        padded, segment,
    };

    /// A well-formed request in wire order; field 3, the card number, is supplied per test.
    const FIELDS: [&[u8]; 18] = [
        b"220618",
        b"143012",
        b"",
        b"0100",
        b"0927",
        b"0200",
        b"POS",
        b"000000",
        b"100.00",
        b"5411",
        b"840",
        b"05",
        b"MERCHANT0000001",
        b"CORNER GROCERY",
        b"SEATTLE",
        b"WA",
        b"98101",
        b"ORDER-A",
    ];

    /// Sixteen digits that no cross-reference record holds.
    const UNKNOWN_CARD: &str = "9999999999999999";

    /// The request body for `card`: [`FIELDS`] joined by commas, each `(field, bytes)` of
    /// `changes`, its field counted from 1, in place of that field.
    fn body(card: &[u8], changes: &[(usize, &[u8])]) -> Vec<u8> {
        let mut fields: Vec<&[u8]> = FIELDS.to_vec();
        if let Some(slot) = fields.get_mut(2) {
            *slot = card;
        }
        for &(position, bytes) in changes {
            if let Some(slot) = position
                .checked_sub(1)
                .and_then(|index| fields.get_mut(index))
            {
                *slot = bytes;
            }
        }
        fields.join(b",".as_slice())
    }

    /// The fixture records CP00 reads, decoded in place.
    struct World {
        xrefs: Vec<CardXrefRecord>,
        xref_lines: Vec<Vec<u8>>,
        accounts: Vec<AccountRecord>,
        customers: Vec<CustomerRecord>,
    }

    impl World {
        fn load() -> Result<Self, Box<dyn Error>> {
            Ok(Self {
                xrefs: fixture_records("cardxref.txt")?,
                xref_lines: fixture_lines("cardxref.txt")?,
                accounts: fixture_records("acctdata.txt")?,
                customers: fixture_records("custdata.txt")?,
            })
        }

        /// The first cross-reference record and the 16 card-number bytes of its line.
        fn first(&self) -> Result<(&CardXrefRecord, &[u8]), Box<dyn Error>> {
            let xref = self.xrefs.first().ok_or("no cross-reference record")?;
            let line = self.xref_lines.first().ok_or("no cross-reference line")?;
            Ok((xref, segment(line, 0, 16)?))
        }

        /// The account of `xref`.
        fn account(&self, xref: &CardXrefRecord) -> Result<&AccountRecord, Box<dyn Error>> {
            self.accounts
                .iter()
                .find(|account| account.id == xref.acct_id)
                .ok_or_else(|| "no fixture account for the cross-reference".into())
        }

        /// The result of the customer read for `xref`.
        fn customer(&self, xref: &CardXrefRecord) -> CustomerLookup {
            if self
                .customers
                .iter()
                .any(|customer| customer.id == xref.cust_id)
            {
                CustomerLookup::Found
            } else {
                CustomerLookup::NotFound
            }
        }
    }

    /// `base` with its balance and credit limit replaced, in cents.
    fn with_balance(
        base: &AccountRecord,
        balance: i64,
        limit: i64,
    ) -> Result<AccountRecord, Box<dyn Error>> {
        Ok(AccountRecord {
            curr_bal: money(balance)?,
            credit_limit: money(limit)?,
            ..base.clone()
        })
    }

    /// The summary of `xref` after earlier authorizations: its limit, credit balance and
    /// approved amount, in cents.
    fn existing(
        xref: &CardXrefRecord,
        limit: i64,
        credit_balance: i64,
        approved: i64,
    ) -> Result<PendingAuthSummaryRecord, Box<dyn Error>> {
        Ok(PendingAuthSummaryRecord {
            credit_limit: money(limit)?,
            credit_balance: money(credit_balance)?,
            approved_auth_amt: money(approved)?,
            ..summary_template(xref)
        })
    }

    /// The request for `card` asking `amount`, the text of field 9.
    fn asking(card: &[u8], amount: &[u8]) -> Result<AuthRequestRecord, Box<dyn Error>> {
        Ok(parse_request(&body(card, &[(9, amount)]))?)
    }

    /// The lookups of a card found in the cross-reference.
    fn found<'a>(
        xref: &'a CardXrefRecord,
        account: Option<&'a AccountRecord>,
        customer: CustomerLookup,
        summary: SummaryState<'a>,
    ) -> Lookup<'a> {
        Lookup::CardFound(FoundContext {
            xref,
            account,
            customer,
            summary,
        })
    }

    /// Asserts a decision's status, reason and guard outcome, and that a decline approves 0.00.
    fn assert_decision(
        decision: &Decision,
        expected: (AuthStatus, ReasonCode, GuardOutcome),
        rule: &str,
    ) {
        let (status, reason, guard) = expected;
        assert_eq!(decision.status, status, "{rule}");
        assert_eq!(decision.reason, reason, "{rule}");
        assert_eq!(decision.guard, guard, "{rule}");
        assert_eq!(
            decision.reply.auth_resp_reason.as_bytes(),
            reason.code(),
            "{rule}"
        );
        if status == AuthStatus::Declined {
            assert!(decision.reply.approved_amt.is_zero(), "{rule}");
        }
    }

    /// The 63 bytes the reply `STRING` writes. The body's length and any byte after these are
    /// `HALTED(H6)` and never read here.
    fn written(reply: &AuthReply) -> Result<Vec<u8>, Box<dyn Error>> {
        Ok(segment(&reply_body(reply), 0, REPLY_WRITTEN_LEN)?.to_vec())
    }

    /// The field error a body gives; any other outcome fails the test.
    fn field_error(bytes: &[u8]) -> Result<FieldError, Box<dyn Error>> {
        match parse_request(bytes) {
            Err(ParseError::Field(error)) => Ok(error),
            Err(ParseError::CardNumber(_)) => Err("a card-number error, not a field error".into()),
            Ok(_) => Err("a request, not a field error".into()),
        }
    }

    /// The fault of the field error a body gives.
    fn fault(bytes: &[u8]) -> Result<FieldFault, Box<dyn Error>> {
        field_error(bytes).map(|error| error.fault)
    }

    /// The response code, reason and amount a reply writes at bytes 40-41, 43-46 and 48-61.
    type ReplyCodes = ([u8; 2], [u8; 4], [u8; 14]);

    /// Fifteen spaces with `len` bytes `X` from `start`: a transaction ID for the screen.
    fn run_at(start: usize, len: usize) -> [u8; 15] {
        let mut id = [b' '; 15];
        for byte in id.iter_mut().skip(start).take(len) {
            *byte = b'X';
        }
        id
    }

    /// An approval that passed the i10 guard: `00`, reason `0000` (COPAUA0C-044, COPAUA0C-046).
    const APPROVED: (AuthStatus, ReasonCode, GuardOutcome) = (
        AuthStatus::Approved,
        ReasonCode::Approved0000,
        GuardOutcome::Passed,
    );
    /// COPAUA0C's own insufficient-funds decline, before any guard: `05`, `4100` (COPAUA0C-045).
    const FUNDS_DECLINED: (AuthStatus, ReasonCode, GuardOutcome) = (
        AuthStatus::Declined,
        ReasonCode::InsufficientFunds4100,
        GuardOutcome::NotApplied,
    );
    /// The i10 guard's decline of an approval that would breach the limit: `05`, `4100` (E30).
    const GUARD_DECLINED: (AuthStatus, ReasonCode, GuardOutcome) = (
        AuthStatus::Declined,
        ReasonCode::InsufficientFunds4100,
        GuardOutcome::DeclinedByCreditLimit,
    );
    /// COPAUA0C's decline when the account or the customer was not found: `05`, `3100`.
    const NOT_FOUND_DECLINED: (AuthStatus, ReasonCode, GuardOutcome) = (
        AuthStatus::Declined,
        ReasonCode::NotFound3100,
        GuardOutcome::NotApplied,
    );

    // audit: COPAUA0C-002
    // audit: COPAUA0C-003
    // audit: COPAUA0C-013
    #[test]
    fn a_well_formed_body_parses_into_the_eighteen_receivers() -> TestResult {
        let world = World::load()?;
        let (xref, card) = world.first()?;
        let request = parse_request(&body(card, &[]))?;
        assert!(request.card_num == xref.card_num, "COPAUA0C-005");
        let alphanumeric: [(&[u8], &[u8]); 14] = [
            (request.auth_date.as_bytes(), b"220618"),
            (request.auth_time.as_bytes(), b"143012"),
            (request.auth_type.as_bytes(), b"0100"),
            (request.card_expiry_date.as_bytes(), b"0927"),
            (request.message_type.as_bytes(), b"0200  "),
            (request.message_source.as_bytes(), b"POS   "),
            (request.merchant_catagory_code.as_bytes(), b"5411"),
            (request.acqr_country_code.as_bytes(), b"840"),
            (request.merchant_id.as_bytes(), b"MERCHANT0000001"),
            (request.merchant_name.as_bytes(), b"CORNER GROCERY        "),
            (request.merchant_city.as_bytes(), b"SEATTLE      "),
            (request.merchant_state.as_bytes(), b"WA"),
            (request.merchant_zip.as_bytes(), b"98101    "),
            (request.transaction_id.as_bytes(), b"ORDER-A        "),
        ];
        for (received, expected) in alphanumeric {
            assert_eq!(received, expected, "COPAUA0C-003");
        }
        assert_eq!(request.processing_code.get(), 0, "COPAUA0C-004");
        assert_eq!(request.pos_entry_mode.get(), 5, "COPAUA0C-004");
        assert!(
            request.transaction_amt == money::<10>(10_000)?,
            "COPAUA0C-014"
        );
        // Every fixture card number parses, so every cross-reference card is a valid request.
        for line in &world.xref_lines {
            let parsed = parse_request(&body(segment(line, 0, 16)?, &[]))?;
            assert!(
                world
                    .xrefs
                    .iter()
                    .any(|xref| xref.card_num == parsed.card_num),
                "COPAUA0C-013"
            );
        }
        Ok(())
    }

    // audit: COPAUA0C-002
    #[test]
    fn the_body_is_cut_at_every_comma_and_nowhere_else() -> TestResult {
        let world = World::load()?;
        let (_, card) = world.first()?;
        let plain = parse_request(&body(card, &[]))?;
        // A final comma ends the last field and opens none; fields after the 18th are ignored.
        let mut trailing = body(card, &[]);
        trailing.push(b',');
        assert!(parse_request(&trailing)? == plain, "COPAUA0C-002");
        let mut extra = body(card, &[]);
        extra.extend_from_slice(b",EXTRA,,MORE,");
        assert!(parse_request(&extra)? == plain, "COPAUA0C-002");
        // Spaces, semicolons, periods and other punctuation are data.
        let punctuated = parse_request(&body(card, &[(14, b" A;B C.D/E ")]))?;
        assert_eq!(
            punctuated.merchant_name.as_bytes(),
            padded(b" A;B C.D/E ", 22).as_slice(),
            "COPAUA0C-002"
        );
        Ok(())
    }

    // audit: COPAUA0C-003
    #[test]
    fn alphanumeric_fields_move_left_justified_padded_and_cut() -> TestResult {
        let world = World::load()?;
        let (_, card) = world.first()?;
        let long = parse_request(&body(card, &[(14, b"ABCDEFGHIJKLMNOPQRSTUVWXY")]))?;
        assert_eq!(
            long.merchant_name.as_bytes(),
            b"ABCDEFGHIJKLMNOPQRSTUV",
            "COPAUA0C-003"
        );
        let empty = parse_request(&body(card, &[(1, b""), (17, b"")]))?;
        assert!(empty.auth_date.is_spaces(), "COPAUA0C-003");
        assert!(empty.merchant_zip.is_spaces(), "COPAUA0C-003");
        // An empty field 18 needs a field after it: a final comma opens no field (COPAUA0C-002).
        let mut empty_last = body(card, &[(18, b"")]);
        empty_last.extend_from_slice(b",19");
        assert!(
            parse_request(&empty_last)?.transaction_id.is_spaces(),
            "COPAUA0C-003"
        );
        let kept = parse_request(&body(card, &[(16, b" w"), (6, b"msg")]))?;
        assert_eq!(kept.merchant_state.as_bytes(), b" w", "COPAUA0C-003");
        assert_eq!(kept.message_type.as_bytes(), b"msg   ", "COPAUA0C-003");
        Ok(())
    }

    // audit: COPAUA0C-004
    #[test]
    fn numeric_fields_keep_their_rightmost_digits() -> TestResult {
        let world = World::load()?;
        let (_, card) = world.first()?;
        let cases: [(usize, &[u8], u64); 6] = [
            (8, b"7", 7),
            (8, b"1234567", 234_567),
            (8, b"", 0),
            (12, b"123", 23),
            (12, b"7", 7),
            (12, b"", 0),
        ];
        for (position, text, expected) in cases {
            let request = parse_request(&body(card, &[(position, text)]))?;
            let value = if position == 8 {
                request.processing_code.get()
            } else {
                request.pos_entry_mode.get()
            };
            assert_eq!(value, expected, "COPAUA0C-004");
        }
        Ok(())
    }

    // audit: COPAUA0C-005
    // audit: COPAUA0C-006
    #[test]
    fn the_card_number_must_be_exactly_sixteen_ascii_digits() -> TestResult {
        let world = World::load()?;
        let (_, card) = world.first()?;
        let fifteen = segment(card, 0, 15)?;
        let cases: [(Vec<u8>, CardNumberCause); 9] = [
            (body(b"ABC", &[]), CardNumberCause::NotSixteenDigits),
            (body(fifteen, &[]), CardNumberCause::NotSixteenDigits),
            (
                body(&[card, b"0"].concat(), &[]),
                CardNumberCause::NotSixteenDigits,
            ),
            (
                body(&[fifteen, b" "].concat(), &[]),
                CardNumberCause::NotSixteenDigits,
            ),
            (
                body(&[b"+", fifteen].concat(), &[]),
                CardNumberCause::NotSixteenDigits,
            ),
            (
                body(&[fifteen, &[0xFF]].concat(), &[]),
                CardNumberCause::NotSixteenDigits,
            ),
            (body(b"", &[]), CardNumberCause::NotSixteenDigits),
            (Vec::new(), CardNumberCause::Absent),
            (b"220618,143012".to_vec(), CardNumberCause::Absent),
        ];
        for (bytes, cause) in &cases {
            let Err(ParseError::CardNumber(error)) = parse_request(bytes) else {
                return Err("COPAUA0C-005: not a card-number error".into());
            };
            assert_eq!(error.cause, *cause, "COPAUA0C-005");
            assert_eq!(error.body_len, bytes.len(), "COPAUA0C-006");
        }
        // The worker's alert names the class and the body length, never a body byte.
        assert_eq!(
            ALERT_CARD_UNPARSEABLE, "CP00_CARD_UNPARSEABLE",
            "COPAUA0C-006"
        );
        let text = parse_request(&body(b"ABCDEFGHIJKLMNOP", &[]))
            .err()
            .map(|error| error.to_string())
            .unwrap_or_default();
        assert!(
            !text.is_empty() && !text.contains("ABCDEFGH") && !text.contains("ORDER-A"),
            "COPAUA0C-006"
        );
        Ok(())
    }

    // audit: COPAUA0C-005
    // audit: COPAUA0C-040
    #[test]
    fn a_card_missing_from_the_cross_reference_is_valid_and_declined() -> TestResult {
        let world = World::load()?;
        let unknown = CardNumber::parse(UNKNOWN_CARD)?;
        assert!(
            world.xrefs.iter().all(|xref| xref.card_num != unknown),
            "COPAUA0C-005"
        );
        let request = parse_request(&body(UNKNOWN_CARD.as_bytes(), &[]))?;
        assert!(request.card_num == unknown, "COPAUA0C-005");
        // HALTED(X-HALTS-1): the COBOL then decides on a summary flag no read set, so no parity
        // value exists. The `05`/`3100` decline is the replacement behaviour AAP 0.3.4.2 states
        // for a well-formed card missing from `card_xref`, asserted as that and never as parity.
        let decision = decide(&request, &Lookup::CardNotInXref);
        assert_decision(&decision, NOT_FOUND_DECLINED, "AAP 0.3.4.2");
        blocked("COPAUA0C-040", "X-HALTS-1");
        Ok(())
    }

    // audit: COPAUA0C-007
    #[test]
    fn fewer_than_eighteen_fields_is_a_field_error() -> TestResult {
        let world = World::load()?;
        let (_, card) = world.first()?;
        let full = body(card, &[]);
        let last_comma = full
            .iter()
            .rposition(|&byte| byte == b',')
            .ok_or("no comma")?;
        let seventeen = segment(&full, 0, last_comma)?;
        assert_eq!(
            fault(seventeen)?,
            FieldFault::Missing(RequestField::TransactionId),
            "COPAUA0C-007"
        );
        let with_final_comma = [seventeen, b","].concat();
        assert_eq!(
            fault(&with_final_comma)?,
            FieldFault::Missing(RequestField::TransactionId),
            "COPAUA0C-007"
        );
        let three = [b"220618,143012,".as_slice(), card].concat();
        assert_eq!(
            fault(&three)?,
            FieldFault::Missing(RequestField::AuthType),
            "COPAUA0C-007"
        );
        Ok(())
    }

    // audit: COPAUA0C-008
    #[test]
    fn the_amount_text_must_be_a_valid_numval_argument() -> TestResult {
        let world = World::load()?;
        let (_, card) = world.first()?;
        let invalid: [&[u8]; 15] = [
            b"", b"     ", b"1 000", b"1.2.3", b"+1-", b"-1CR", b"+", b"+.", b".", b"1cr", b"1db",
            b"12A", b"$5", b"1E5", b"--1",
        ];
        for text in invalid {
            assert_eq!(
                fault(&body(card, &[(9, text)]))?,
                FieldFault::AmountNotNumeric,
                "COPAUA0C-008"
            );
        }
        let valid: [&[u8]; 10] = [
            b"100", b"  100  ", b"+ 7", b"- 7", b"7 -", b"7CR", b"7 DB", b".5", b"5.", b"0",
        ];
        for text in valid {
            assert!(
                parse_request(&body(card, &[(9, text)])).is_ok(),
                "COPAUA0C-008"
            );
        }
        Ok(())
    }

    // audit: COPAUA0C-009
    // audit: COPAUA0C-010
    // audit: COPAUA0C-011
    #[test]
    fn malformed_numeric_fields_long_bodies_and_control_bytes_are_field_errors() -> TestResult {
        let world = World::load()?;
        let (_, card) = world.first()?;
        let non_digits: [(usize, &[u8], RequestField); 6] = [
            (8, b"12A", RequestField::ProcessingCode),
            (8, b"-1", RequestField::ProcessingCode),
            (8, b" 5", RequestField::ProcessingCode),
            (8, b"1.0", RequestField::ProcessingCode),
            (12, b"0x", RequestField::PosEntryMode),
            (12, b"+5", RequestField::PosEntryMode),
        ];
        for (position, text, field) in non_digits {
            assert_eq!(
                fault(&body(card, &[(position, text)]))?,
                FieldFault::NonDigit(field),
                "COPAUA0C-009"
            );
        }
        // Up to the 500-byte buffer the body is a request; one byte more is a field error.
        let base_len = body(card, &[]).len();
        let filler = |extra: usize| vec![b'N'; FIELDS.get(13).map_or(0, |name| name.len()) + extra];
        let at_limit = filler(GET_BUFFER_LEN - base_len);
        assert!(
            parse_request(&body(card, &[(14, &at_limit)])).is_ok(),
            "COPAUA0C-010"
        );
        let over = filler(GET_BUFFER_LEN - base_len + 1);
        assert_eq!(
            fault(&body(card, &[(14, &over)]))?,
            FieldFault::BodyTooLong {
                len: GET_BUFFER_LEN + 1
            },
            "COPAUA0C-010"
        );
        let control: [(usize, &[u8], RequestField); 4] = [
            (1, b"22\x0018", RequestField::AuthDate),
            (2, b"14\x7F012", RequestField::AuthTime),
            (4, b"01\xC3\xA9", RequestField::AuthType),
            (18, b"ORDER\tA", RequestField::TransactionId),
        ];
        for (position, text, field) in control {
            assert_eq!(
                fault(&body(card, &[(position, text)]))?,
                FieldFault::NonPrintable(field),
                "COPAUA0C-011"
            );
        }
        Ok(())
    }

    // audit: COPAUA0C-012
    #[test]
    fn a_field_error_gets_a_05_9000_decline_with_no_amount() -> TestResult {
        let world = World::load()?;
        let (xref, card) = world.first()?;
        let error = field_error(&body(card, &[(9, b"ABC")]))?;
        assert!(error.card == xref.card_num, "COPAUA0C-012");
        let reply = field_error_reply(&error);
        assert_eq!(reply.auth_resp_code.as_bytes(), b"05", "COPAUA0C-012");
        assert_eq!(reply.auth_resp_reason.as_bytes(), b"9000", "COPAUA0C-012");
        assert!(reply.approved_amt.is_zero(), "COPAUA0C-012");
        let bytes = written(&reply)?;
        // Card-bearing bytes are compared unprinted (Cardholder Data Redaction).
        assert!(segment(&bytes, 0, 12)? == b"************", "COPAUA0C-012");
        assert!(
            segment(&bytes, 12, 4)? == segment(card, 12, 4)?,
            "COPAUA0C-012"
        );
        assert_eq!(segment(&bytes, 17, 15)?, b"ORDER-A        ", "COPAUA0C-012");
        assert_eq!(segment(&bytes, 33, 6)?, b"143012", "COPAUA0C-012");
        assert_eq!(segment(&bytes, 40, 2)?, b"05", "COPAUA0C-012");
        assert_eq!(segment(&bytes, 43, 4)?, b"9000", "COPAUA0C-012");
        assert_eq!(segment(&bytes, 48, 14)?, b"          0.00", "COPAUA0C-012");
        // With fewer than 18 fields the transaction-ID field is 15 spaces.
        let short = field_error(&[b"220618,143012,".as_slice(), card, b",0100"].concat())?;
        assert!(short.transaction_id.is_spaces(), "COPAUA0C-012");
        let short_reply = written(&field_error_reply(&short))?;
        assert_eq!(segment(&short_reply, 17, 15)?, [b' '; 15], "COPAUA0C-012");
        assert_eq!(segment(&short_reply, 33, 6)?, b"143012", "COPAUA0C-012");
        // A field 18 of eight or more consecutive non-space bytes is withheld (X-COPAUA0C-11).
        let probe = field_error(&body(card, &[(9, b"ABC"), (18, b"123456789")]))?;
        let probe_reply = written(&field_error_reply(&probe))?;
        assert_eq!(segment(&probe_reply, 17, 15)?, [b'*'; 15], "COPAUA0C-012");
        Ok(())
    }

    // audit: COPAUA0C-014
    #[test]
    fn the_amount_is_the_exact_value_of_its_thirteen_bytes() -> TestResult {
        let world = World::load()?;
        let (_, card) = world.first()?;
        let cases: [(&[u8], i64); 11] = [
            (b"12345678901.5", 234_567_890_150),
            (b"+0000000123.45", 12_340),
            (b"  100", 10_000),
            (b"5.25-", -525),
            (b"5.25CR", -525),
            (b"5.25 DB", -525),
            (b"- 7", -700),
            (b".5", 50),
            (b"100.", 10_000),
            (b"0.01", 1),
            (b"-12345678901", -234_567_890_100),
        ];
        for (text, cents) in cases {
            let request = asking(card, text)?;
            assert!(
                request.transaction_amt == money::<10>(cents)?,
                "COPAUA0C-014"
            );
        }
        Ok(())
    }

    // audit: COPAUA0C-015
    // audit: COPAUA0C-016
    #[test]
    fn excess_decimals_and_a_minus_zero_are_blocked() -> TestResult {
        let world = World::load()?;
        let (_, card) = world.first()?;
        // Each text is a valid NUMVAL argument, so it makes a request (COPAUA0C-008). Its stored
        // value is HALTED(X-ARITH-5) or HALTED(X-COPAUA0C-H3) and is not asserted.
        for text in [b"1.005".as_slice(), b".125", b"99.999"] {
            assert!(asking(card, text).is_ok(), "COPAUA0C-008");
        }
        for text in [b"-0".as_slice(), b"-0.00", b"0-", b"0CR", b".00DB"] {
            assert!(asking(card, text).is_ok(), "COPAUA0C-008");
        }
        blocked("COPAUA0C-015", "X-ARITH-5");
        blocked("COPAUA0C-016", "X-COPAUA0C-H3");
        Ok(())
    }

    // audit: COPAUA0C-017
    #[test]
    fn the_card_check_runs_before_every_field_check() -> TestResult {
        let world = World::load()?;
        let (_, card) = world.first()?;
        let both = body(b"ABC", &[(9, b"NOT A NUMBER"), (8, b"X")]);
        assert!(
            matches!(parse_request(&both), Err(ParseError::CardNumber(_))),
            "COPAUA0C-017"
        );
        let long = vec![b'N'; GET_BUFFER_LEN];
        let long_and_bad_card = body(b"1234", &[(14, &long)]);
        assert!(
            matches!(
                parse_request(&long_and_bad_card),
                Err(ParseError::CardNumber(_))
            ),
            "COPAUA0C-017"
        );
        let long_and_good_card = body(card, &[(14, &long)]);
        assert!(
            matches!(
                parse_request(&long_and_good_card),
                Err(ParseError::Field(_))
            ),
            "COPAUA0C-017"
        );
        Ok(())
    }

    // audit: COPAUA0C-041
    // audit: ARITH-034
    // audit: ARITH-037
    #[test]
    fn an_existing_summary_offers_its_limit_less_its_credit_balance() -> TestResult {
        let world = World::load()?;
        let (xref, card) = world.first()?;
        let account = world.account(xref)?;
        // An account with room under the guard, so only COPAUA0C's own test can decline.
        let roomy = with_balance(account, 0, 1_000_000)?;
        let row = existing(xref, 500_000, 120_000, 0)?;
        let lookup = found(
            xref,
            Some(&roomy),
            CustomerLookup::Found,
            SummaryState::Existing(&row),
        );
        // 5000.00 − 1200.00 = 3800.00 available: the amount must not exceed it.
        assert_decision(
            &decide(&asking(card, b"3800.00")?, &lookup),
            APPROVED,
            "ARITH-037",
        );
        assert_decision(
            &decide(&asking(card, b"3800.01")?, &lookup),
            FUNDS_DECLINED,
            "ARITH-037",
        );
        // `ACCT-CURR-BAL` plays no part (E30): a balance at the limit leaves this test as it is,
        // and only the guard that follows sees it.
        let full = with_balance(account, 999_999, 1_000_000)?;
        let crowded = found(
            xref,
            Some(&full),
            CustomerLookup::Found,
            SummaryState::Existing(&row),
        );
        let within = decide(&asking(card, b"3800.00")?, &crowded);
        assert_ne!(within.guard, GuardOutcome::NotApplied, "COPAUA0C-041");
        let beyond = decide(&asking(card, b"3800.01")?, &crowded);
        assert_decision(&beyond, FUNDS_DECLINED, "COPAUA0C-041");
        // 999999999.99 − (−0.01) = 1000000000.00 loses its high-order digit: 0.00 available.
        let wrapped = existing(xref, 99_999_999_999, -1, 0)?;
        let lookup = found(
            xref,
            Some(&roomy),
            CustomerLookup::Found,
            SummaryState::Existing(&wrapped),
        );
        assert_decision(
            &decide(&asking(card, b"0.01")?, &lookup),
            FUNDS_DECLINED,
            "ARITH-034",
        );
        assert_decision(
            &decide(&asking(card, b"0.00")?, &lookup),
            APPROVED,
            "ARITH-034",
        );
        Ok(())
    }

    // audit: COPAUA0C-042
    // audit: ARITH-035
    // audit: ARITH-036
    #[test]
    fn a_new_summary_offers_the_account_limit_less_its_balance() -> TestResult {
        let world = World::load()?;
        let (xref, card) = world.first()?;
        let account = world.account(xref)?;
        let template = summary_template(xref);
        let created = SummaryState::Created(&template);
        let customer = world.customer(xref);
        // 5000.00 − 843.00 = 4157.00 available.
        let limited = with_balance(account, 84_300, 500_000)?;
        let lookup = found(xref, Some(&limited), customer, created);
        assert_decision(
            &decide(&asking(card, b"4157.00")?, &lookup),
            APPROVED,
            "ARITH-035",
        );
        assert_decision(
            &decide(&asking(card, b"4157.01")?, &lookup),
            FUNDS_DECLINED,
            "ARITH-035",
        );
        // 1500000000.00 − 200.00 is stored 499999800.00, so 600000000.00 is declined (E12).
        let wide = with_balance(account, 20_000, 150_000_000_000)?;
        let lookup = found(xref, Some(&wide), customer, created);
        assert_decision(
            &decide(&asking(card, b"600000000.00")?, &lookup),
            FUNDS_DECLINED,
            "ARITH-036",
        );
        // 1000.00 − (−999999500.00) is stored 500.00.
        let credit = with_balance(account, -99_999_950_000, 100_000)?;
        let lookup = found(xref, Some(&credit), customer, created);
        assert_decision(
            &decide(&asking(card, b"500.01")?, &lookup),
            FUNDS_DECLINED,
            "ARITH-036",
        );
        assert_decision(
            &decide(&asking(card, b"500.00")?, &lookup),
            APPROVED,
            "ARITH-036",
        );
        Ok(())
    }

    // audit: COPAUA0C-043
    #[test]
    fn a_new_summary_without_its_account_is_declined_3100() -> TestResult {
        let world = World::load()?;
        let (xref, card) = world.first()?;
        let template = summary_template(xref);
        let lookup = found(
            xref,
            None,
            world.customer(xref),
            SummaryState::Created(&template),
        );
        for amount in [b"0.00".as_slice(), b"100.00", b"999999.00"] {
            assert_decision(
                &decide(&asking(card, amount)?, &lookup),
                NOT_FOUND_DECLINED,
                "COPAUA0C-043",
            );
        }
        Ok(())
    }

    // audit: COPAUA0C-044
    // audit: ARITH-038
    #[test]
    fn an_approval_carries_the_request_amount_and_a_decline_zero() -> TestResult {
        let world = World::load()?;
        let (xref, card) = world.first()?;
        let account = world.account(xref)?;
        let template = summary_template(xref);
        let lookup = found(
            xref,
            Some(account),
            world.customer(xref),
            SummaryState::Created(&template),
        );
        let request = asking(card, b"100.00")?;
        let approved = decide(&request, &lookup);
        assert_decision(&approved, APPROVED, "COPAUA0C-044");
        assert_eq!(
            approved.reply.auth_resp_code.as_bytes(),
            b"00",
            "COPAUA0C-044"
        );
        assert!(
            approved.reply.approved_amt == request.transaction_amt,
            "ARITH-038"
        );
        assert!(
            approved.reply.approved_amt == money::<10>(10_000)?,
            "ARITH-038"
        );
        assert!(
            approved.transaction_amt == request.transaction_amt,
            "COPAUA0C-044"
        );
        let refused = asking(card, b"999999.00")?;
        let declined = decide(&refused, &lookup);
        assert_decision(&declined, FUNDS_DECLINED, "COPAUA0C-044");
        assert_eq!(
            declined.reply.auth_resp_code.as_bytes(),
            b"05",
            "COPAUA0C-044"
        );
        assert!(
            declined.transaction_amt == refused.transaction_amt,
            "COPAUA0C-044"
        );
        Ok(())
    }

    // audit: COPAUA0C-045
    // audit: ARITH-037
    #[test]
    fn a_failed_lookup_outranks_insufficient_funds_in_the_reason() -> TestResult {
        let world = World::load()?;
        let (xref, card) = world.first()?;
        let account = world.account(xref)?;
        let roomy = with_balance(account, 0, 1_000_000)?;
        let row = existing(xref, 500_000, 120_000, 0)?;
        let summary = SummaryState::Existing(&row);
        let over = asking(card, b"3800.01")?;
        let within = asking(card, b"3800.00")?;
        // Insufficient funds with the customer not found: `3100`.
        let no_customer = found(xref, Some(&roomy), CustomerLookup::NotFound, summary);
        assert_decision(
            &decide(&over, &no_customer),
            NOT_FOUND_DECLINED,
            "COPAUA0C-045",
        );
        // Insufficient funds on the summary path with the account not found: `3100`.
        let no_account = found(xref, None, CustomerLookup::Found, summary);
        assert_decision(
            &decide(&over, &no_account),
            NOT_FOUND_DECLINED,
            "COPAUA0C-045",
        );
        // Insufficient funds with the account and the customer found: `4100`.
        let both = found(xref, Some(&roomy), CustomerLookup::Found, summary);
        assert_decision(&decide(&over, &both), FUNDS_DECLINED, "COPAUA0C-045");
        // A customer not found never declines by itself.
        assert_decision(&decide(&within, &no_customer), APPROVED, "COPAUA0C-045");
        // The reasons a reply can carry; 4200 to 5200 are unreachable (E13).
        let codes: [(ReasonCode, &[u8; 4]); 4] = [
            (ReasonCode::Approved0000, b"0000"),
            (ReasonCode::NotFound3100, b"3100"),
            (ReasonCode::InsufficientFunds4100, b"4100"),
            (ReasonCode::Other9000, b"9000"),
        ];
        for (reason, code) in codes {
            assert_eq!(reason.code(), code, "COPAUA0C-045");
        }
        Ok(())
    }

    // audit: COPAUA0C-046
    // audit: COPAUA0C-049
    // audit: ARITH-045
    #[test]
    fn the_guard_declines_an_approval_that_would_breach_the_credit_limit() -> TestResult {
        let world = World::load()?;
        let (xref, card) = world.first()?;
        // Balance 0.00 and limit 2450.00; the summary's own test offers 5000.00, so only the
        // guard can decline a request of 100.00.
        let account = with_balance(world.account(xref)?, 0, 245_000)?;
        let request = asking(card, b"100.00")?;
        let decide_with = |approved: i64| -> Result<Decision, Box<dyn Error>> {
            let row = existing(xref, 500_000, 0, approved)?;
            let lookup = found(
                xref,
                Some(&account),
                CustomerLookup::Found,
                SummaryState::Existing(&row),
            );
            Ok(decide(&request, &lookup))
        };
        // 0.00 + 2400.00 + 100.00 = 2500.00 > 2450.00.
        assert_decision(&decide_with(240_000)?, GUARD_DECLINED, "ARITH-045");
        // 0.00 + 2300.00 + 100.00 = 2400.00 ≤ 2450.00.
        assert_decision(&decide_with(230_000)?, APPROVED, "ARITH-045");
        // Equality approves: 0.00 + 2350.00 + 100.00 = 2450.00.
        assert_decision(&decide_with(235_000)?, APPROVED, "COPAUA0C-046");
        // The guard reads the summary's approved amount, never its credit balance: a credit
        // balance of 2400.00 with no approved amount leaves room (COPAUA0C-049).
        let row = existing(xref, 500_000, 240_000, 0)?;
        let lookup = found(
            xref,
            Some(&account),
            CustomerLookup::Found,
            SummaryState::Existing(&row),
        );
        assert_decision(&decide(&request, &lookup), APPROVED, "COPAUA0C-049");
        // Every COBOL decline is kept: 5000.00 − 4950.00 leaves 50.00, and the guard never runs.
        let row = existing(xref, 500_000, 495_000, 0)?;
        let lookup = found(
            xref,
            Some(&account),
            CustomerLookup::Found,
            SummaryState::Existing(&row),
        );
        assert_decision(&decide(&request, &lookup), FUNDS_DECLINED, "COPAUA0C-046");
        Ok(())
    }

    // audit: COPAUA0C-047
    #[test]
    fn a_guard_decline_with_the_customer_not_found_is_4100() -> TestResult {
        let world = World::load()?;
        let (xref, card) = world.first()?;
        let account = with_balance(world.account(xref)?, 0, 245_000)?;
        let row = existing(xref, 500_000, 0, 240_000)?;
        let lookup = found(
            xref,
            Some(&account),
            CustomerLookup::NotFound,
            SummaryState::Existing(&row),
        );
        assert_decision(
            &decide(&asking(card, b"100.00")?, &lookup),
            GUARD_DECLINED,
            "COPAUA0C-047",
        );
        Ok(())
    }

    // audit: COPAUA0C-048
    #[test]
    fn an_existing_summary_without_its_account_is_declined_3100() -> TestResult {
        let world = World::load()?;
        let (xref, card) = world.first()?;
        let row = existing(xref, 500_000, 120_000, 0)?;
        let lookup = found(
            xref,
            None,
            CustomerLookup::Found,
            SummaryState::Existing(&row),
        );
        // COPAUA0C's own test approves 3800.00, but the guard has no balance or limit to bound it.
        assert_decision(
            &decide(&asking(card, b"3800.00")?, &lookup),
            (
                AuthStatus::Declined,
                ReasonCode::NotFound3100,
                GuardOutcome::DeclinedWithoutAccount,
            ),
            "COPAUA0C-048",
        );
        Ok(())
    }

    // audit: COPAUA0C-046
    // audit: COPAUA0C-094
    // audit: COPAUA0C-095
    #[test]
    fn approvals_on_one_summary_stop_at_the_credit_limit() -> TestResult {
        let world = World::load()?;
        let (xref, card) = world.first()?;
        let account = world.account(xref)?;
        // A = 100.00 and L − B = 2450.00 over the fixture balance B.
        let headroom = money::<10>(245_000)?;
        let limited = AccountRecord {
            credit_limit: (account.curr_bal + headroom).store::<10>(),
            ..account.clone()
        };
        let request = asking(card, b"100.00")?;
        let customer = world.customer(xref);
        let template = summary_template(xref);
        let mut row: Option<PendingAuthSummaryRecord> = None;
        let mut approvals = 0_u32;
        for _ in 0..50 {
            let state = row
                .as_ref()
                .map_or(SummaryState::Created(&template), SummaryState::Existing);
            let decision = decide(&request, &found(xref, Some(&limited), customer, state));
            if decision.status == AuthStatus::Approved {
                approvals += 1;
            } else {
                assert_decision(&decision, GUARD_DECLINED, "COPAUA0C-046");
            }
            row = Some(summary_update(state, xref, Some(&limited), &decision)?);
        }
        // k = ⌊(L − B) / A⌋ = 24: the 25th request and every one after it is declined.
        let k = (headroom.as_decimal() / request.transaction_amt.as_decimal()).floor();
        assert_eq!(Decimal::from(approvals), k, "COPAUA0C-046");
        assert_eq!(approvals, 24, "COPAUA0C-046");
        let row = row.ok_or("no summary")?;
        assert_eq!(row.approved_auth_cnt.get(), 24, "COPAUA0C-094");
        assert_eq!(row.declined_auth_cnt.get(), 26, "COPAUA0C-095");
        assert!(
            row.approved_auth_amt == money::<9>(240_000)?,
            "COPAUA0C-094"
        );
        // i10 in the final state: balance plus approved amount within the limit.
        assert!(
            (limited.curr_bal + row.approved_auth_amt).store::<12>() <= limited.credit_limit,
            "COPAUA0C-046"
        );
        Ok(())
    }

    // audit: COPAUA0C-060
    // audit: COPAUA0C-061
    // audit: COPAUA0C-063
    #[test]
    fn the_reply_writes_its_fields_in_ccpaurly_order_with_commas() -> TestResult {
        let world = World::load()?;
        let (xref, card) = world.first()?;
        let template = summary_template(xref);
        let lookup = found(
            xref,
            Some(world.account(xref)?),
            world.customer(xref),
            SummaryState::Created(&template),
        );
        assert_eq!(REPLY_WRITTEN_LEN, 63, "COPAUA0C-060");
        let approved = decide(&asking(card, b"100.00")?, &lookup);
        let declined = decide(&asking(card, b"999999.00")?, &lookup);
        let cases: [(&Decision, ReplyCodes); 2] = [
            (&approved, (*b"00", *b"0000", *b"        100.00")),
            (&declined, (*b"05", *b"4100", *b"          0.00")),
        ];
        for (decision, (response, reason, amount)) in cases {
            let bytes = written(&decision.reply)?;
            for comma in [16, 32, 39, 42, 47, 62] {
                assert_eq!(segment(&bytes, comma, 1)?, b",", "COPAUA0C-060");
            }
            // Twelve `*` and the card's last four digits, never the full number (E32).
            // Card-bearing bytes are compared unprinted (Cardholder Data Redaction).
            assert!(segment(&bytes, 0, 12)? == b"************", "COPAUA0C-061");
            assert!(
                segment(&bytes, 12, 4)? == segment(card, 12, 4)?,
                "COPAUA0C-061"
            );
            assert!(
                !contains(&reply_body(&decision.reply), card),
                "COPAUA0C-061"
            );
            assert_eq!(segment(&bytes, 17, 15)?, b"ORDER-A        ", "COPAUA0C-060");
            assert_eq!(segment(&bytes, 33, 6)?, b"143012", "COPAUA0C-060");
            assert_eq!(segment(&bytes, 40, 2)?, response, "COPAUA0C-063");
            assert_eq!(segment(&bytes, 43, 4)?, reason, "COPAUA0C-063");
            assert_eq!(segment(&bytes, 48, 14)?, amount, "COPAUA0C-060");
        }
        Ok(())
    }

    // audit: COPAUA0C-062
    #[test]
    fn the_transaction_id_screen_withholds_runs_of_eight_non_space_bytes() -> TestResult {
        let passing: [&[u8; 15]; 4] = [
            b"               ",
            b"ORDER-A        ",
            b"ORDER-1 PART-22",
            b"A B C D E F G H",
        ];
        for id in passing {
            assert_eq!(
                &transaction_id_screen(&PicX::from_array(*id)),
                id,
                "COPAUA0C-062"
            );
        }
        for start in 0..=8 {
            let id = run_at(start, 7);
            assert_eq!(
                transaction_id_screen(&PicX::from_array(id)),
                id,
                "COPAUA0C-062"
            );
        }
        for start in 0..=7 {
            let id = run_at(start, 8);
            assert_eq!(
                transaction_id_screen(&PicX::from_array(id)),
                [b'*'; 15],
                "COPAUA0C-062"
            );
        }
        let withheld: [&[u8]; 4] = [
            b"123456789012345",
            b"123-45-6789",
            b"00123456789",
            b"PASSWORD",
        ];
        for id in withheld {
            assert_eq!(
                transaction_id_screen(&PicX::moved_from(id)),
                [b'*'; 15],
                "COPAUA0C-062"
            );
        }
        // Through the reply: field 18 at bytes 17-31, screened; field 2 at 33-38, unscreened.
        let world = World::load()?;
        let (xref, card) = world.first()?;
        let template = summary_template(xref);
        let lookup = found(
            xref,
            Some(world.account(xref)?),
            world.customer(xref),
            SummaryState::Created(&template),
        );
        let request = parse_request(&body(card, &[(2, b"ABCDEF"), (18, b"123-45-6789")]))?;
        let bytes = written(&decide(&request, &lookup).reply)?;
        assert_eq!(segment(&bytes, 17, 15)?, [b'*'; 15], "COPAUA0C-062");
        assert_eq!(segment(&bytes, 33, 6)?, b"ABCDEF", "COPAUA0C-062");
        Ok(())
    }

    // audit: COPAUA0C-064
    // audit: ARITH-039
    #[test]
    fn the_approved_amount_is_edited_then_screened() -> TestResult {
        let cases: [(i64, &[u8; 14]); 6] = [
            (0, b"          0.00"),
            (10_000, b"        100.00"),
            (-525, b"-         5.25"),
            (999_999_999, b"    9999999.99"),
            (1_000_000_000, b"**************"),
            (-1_000_000_000, b"**************"),
        ];
        for (cents, expected) in cases {
            assert_eq!(&amount_screen(money(cents)?), expected, "COPAUA0C-064");
        }
        // The edit keeps every digit; the screen withholds an edit with eight or more of them.
        let ten_digits = Decimal::new(123_456_789_012, 2);
        assert_eq!(
            EditedPicture::MinusZ10Dot99.format(ten_digits),
            " 1234567890.12",
            "ARITH-039"
        );
        assert_eq!(
            &amount_screen(Money::new(ten_digits)?),
            b"**************",
            "COPAUA0C-064"
        );
        // Through an approval: the request amount reaches bytes 48-61 through the screen.
        let world = World::load()?;
        let (xref, card) = world.first()?;
        let rich = with_balance(world.account(xref)?, 0, 999_999_999_999)?;
        let template = summary_template(xref);
        let lookup = found(
            xref,
            Some(&rich),
            CustomerLookup::Found,
            SummaryState::Created(&template),
        );
        let cases: [(&[u8], &[u8; 14]); 2] = [
            (b"9999999.99", b"    9999999.99"),
            (b"10000000.00", b"**************"),
        ];
        for (text, expected) in cases {
            let decision = decide(&asking(card, text)?, &lookup);
            assert_decision(&decision, APPROVED, "COPAUA0C-064");
            assert_eq!(
                segment(&written(&decision.reply)?, 48, 14)?,
                expected,
                "COPAUA0C-064"
            );
        }
        Ok(())
    }

    // audit: COPAUA0C-065
    #[test]
    fn the_reply_length_and_its_last_byte_are_blocked() -> TestResult {
        let world = World::load()?;
        let (xref, card) = world.first()?;
        let template = summary_template(xref);
        let lookup = found(
            xref,
            Some(world.account(xref)?),
            world.customer(xref),
            SummaryState::Created(&template),
        );
        // HALTED(H6): the COBOL sends one byte after the 63 written ones, from storage no source
        // sets. The body is built for coverage; its length and any byte past offset 62 are never
        // asserted. Cardholder Data Redaction holds whatever H6 leaves open: no full card number.
        let body = reply_body(&decide(&asking(card, b"100.00")?, &lookup).reply);
        assert!(!contains(&body, card), "Cardholder Data Redaction");
        blocked("COPAUA0C-065", "H6");
        Ok(())
    }

    // audit: COPAUA0C-027
    // audit: COPAUA0C-092
    // audit: ARITH-041
    #[test]
    fn a_new_summary_starts_from_zeros_under_the_cross_reference_keys() -> TestResult {
        let world = World::load()?;
        for xref in &world.xrefs {
            let row = summary_template(xref);
            assert_eq!(row.acct_id, PicS::from(xref.acct_id), "COPAUA0C-027");
            assert_eq!(
                PicU::<11>::try_from(row.acct_id)?,
                xref.acct_id,
                "COPAUA0C-027"
            );
            assert_eq!(row.cust_id, xref.cust_id, "COPAUA0C-092");
            let amounts = [
                row.credit_limit,
                row.cash_limit,
                row.credit_balance,
                row.cash_balance,
                row.approved_auth_amt,
                row.declined_auth_amt,
            ];
            assert!(amounts.iter().all(|amount| amount.is_zero()), "ARITH-041");
            assert_eq!(row.approved_auth_cnt.get(), 0, "ARITH-041");
            assert_eq!(row.declined_auth_cnt.get(), 0, "ARITH-041");
        }
        // The signed key converts back to the unsigned account key only when it is not negative.
        assert!(
            PicU::<11>::try_from(PicS::<11>::new(-1)?).is_err(),
            "COPAUA0C-027"
        );
        Ok(())
    }

    // audit: COPAUA0C-090
    // audit: COPAUA0C-094
    // audit: ARITH-040
    // audit: ARITH-042
    // audit: ARITH-043
    #[test]
    fn an_approval_adds_to_the_approved_count_amount_and_credit_balance() -> TestResult {
        let world = World::load()?;
        let (xref, card) = world.first()?;
        // A cash limit of 1234567890.12 loses its high-order digit in `S9(09)V99`.
        let account = AccountRecord {
            curr_bal: Money::ZERO,
            credit_limit: money(500_000)?,
            cash_credit_limit: money(123_456_789_012)?,
            ..world.account(xref)?.clone()
        };
        let template = summary_template(xref);
        let created = SummaryState::Created(&template);
        let decision = decide(
            &asking(card, b"100.00")?,
            &found(xref, Some(&account), CustomerLookup::Found, created),
        );
        assert_decision(&decision, APPROVED, "COPAUA0C-094");
        let row = summary_update(created, xref, Some(&account), &decision)?;
        assert_eq!(row.acct_id, PicS::from(xref.acct_id), "COPAUA0C-092");
        assert_eq!(row.cust_id, xref.cust_id, "COPAUA0C-092");
        assert!(row.credit_limit == money::<9>(500_000)?, "ARITH-040");
        assert!(row.cash_limit == money::<9>(23_456_789_012)?, "ARITH-040");
        assert_eq!(row.approved_auth_cnt.get(), 1, "ARITH-042");
        assert!(row.approved_auth_amt == money::<9>(10_000)?, "ARITH-043");
        assert!(row.credit_balance == money::<9>(10_000)?, "ARITH-043");
        assert!(row.cash_balance.is_zero(), "ARITH-043");
        assert_eq!(row.declined_auth_cnt.get(), 0, "COPAUA0C-094");
        assert!(row.declined_auth_amt.is_zero(), "COPAUA0C-094");
        // On an existing summary: 23 becomes 24, 1200.00 becomes 1300.00, and a credit balance
        // of 999999950.00 + 100.00 loses its high-order digit: 50.00.
        let earlier = PendingAuthSummaryRecord {
            approved_auth_cnt: Count::new(23),
            declined_auth_cnt: Count::new(7),
            approved_auth_amt: money(120_000)?,
            credit_balance: money(99_999_995_000)?,
            cash_balance: money(500)?,
            ..template.clone()
        };
        let row = summary_update(
            SummaryState::Existing(&earlier),
            xref,
            Some(&account),
            &decision,
        )?;
        assert_eq!(row.approved_auth_cnt.get(), 24, "ARITH-042");
        assert!(row.approved_auth_amt == money::<9>(130_000)?, "ARITH-043");
        assert!(row.credit_balance == money::<9>(5_000)?, "ARITH-043");
        assert!(row.cash_balance.is_zero(), "ARITH-043");
        assert_eq!(row.declined_auth_cnt.get(), 7, "COPAUA0C-094");
        Ok(())
    }

    // audit: COPAUA0C-090
    // audit: COPAUA0C-095
    // audit: ARITH-042
    #[test]
    fn a_decline_adds_to_the_declined_count_only() -> TestResult {
        let world = World::load()?;
        let (xref, card) = world.first()?;
        let account = world.account(xref)?;
        let template = summary_template(xref);
        let earlier = PendingAuthSummaryRecord {
            approved_auth_cnt: Count::new(5),
            declined_auth_cnt: Count::new(23),
            approved_auth_amt: money(120_000)?,
            credit_balance: money(130_000)?,
            cash_balance: money(500)?,
            ..template.clone()
        };
        let state = SummaryState::Existing(&earlier);
        let decision = decide(
            &asking(card, b"999999.00")?,
            &found(xref, Some(account), CustomerLookup::Found, state),
        );
        assert_eq!(decision.status, AuthStatus::Declined, "COPAUA0C-095");
        let row = summary_update(state, xref, Some(account), &decision)?;
        assert_eq!(row.declined_auth_cnt.get(), 24, "ARITH-042");
        assert_eq!(row.approved_auth_cnt.get(), 5, "COPAUA0C-095");
        assert!(
            row.approved_auth_amt == earlier.approved_auth_amt,
            "COPAUA0C-095"
        );
        assert!(row.credit_balance == earlier.credit_balance, "COPAUA0C-095");
        assert!(row.cash_balance == earlier.cash_balance, "COPAUA0C-095");
        // The limits come from the found account, approved or declined.
        assert!(
            row.credit_limit == MoneyExpr::from(account.credit_limit).store::<9>(),
            "COPAUA0C-090"
        );
        assert!(
            row.cash_limit == MoneyExpr::from(account.cash_credit_limit).store::<9>(),
            "COPAUA0C-090"
        );
        Ok(())
    }

    // audit: COPAUA0C-091
    // audit: COPAUA0C-096
    // audit: ARITH-044
    #[test]
    fn the_declined_amount_and_the_limits_without_an_account_are_blocked() -> TestResult {
        let world = World::load()?;
        let (xref, card) = world.first()?;
        let template = summary_template(xref);
        let state = SummaryState::Created(&template);
        // HALTED(H2): the declined amount adds a field read before its first assignment.
        // HALTED(X-COPAUA0C-H2): with no account the COBOL moves unread storage into the limits.
        // Both summaries are built for coverage; neither field is asserted.
        let declined = decide(
            &asking(card, b"999999.00")?,
            &found(
                xref,
                Some(world.account(xref)?),
                CustomerLookup::Found,
                state,
            ),
        );
        summary_update(state, xref, Some(world.account(xref)?), &declined)?;
        let missing = decide(
            &asking(card, b"100.00")?,
            &found(xref, None, CustomerLookup::Found, state),
        );
        summary_update(state, xref, None, &missing)?;
        blocked("COPAUA0C-096", "H2");
        blocked("ARITH-044", "H2");
        blocked("COPAUA0C-091", "X-COPAUA0C-H2");
        Ok(())
    }

    #[test]
    fn a_count_with_no_room_builds_no_summary() -> TestResult {
        // No rule row: a count of 9999 has no expected next value (X-ARITH-6), so the
        // replacement refuses to build the summary rather than pick one. Not a parity result.
        let world = World::load()?;
        let (xref, card) = world.first()?;
        let account = world.account(xref)?;
        let full = PendingAuthSummaryRecord {
            approved_auth_cnt: Count::new(9_999),
            declined_auth_cnt: Count::new(9_999),
            ..summary_template(xref)
        };
        let state = SummaryState::Existing(&full);
        let template = summary_template(xref);
        let created = SummaryState::Created(&template);
        let lookup = found(xref, Some(account), CustomerLookup::Found, created);
        let approved = decide(&asking(card, b"100.00")?, &lookup);
        let declined = decide(&asking(card, b"999999.00")?, &lookup);
        assert_eq!(
            summary_update(state, xref, Some(account), &approved).err(),
            Some(SummaryError::ApprovedCountAtLimit),
            "X-ARITH-6"
        );
        assert_eq!(
            summary_update(state, xref, Some(account), &declined).err(),
            Some(SummaryError::DeclinedCountAtLimit),
            "X-ARITH-6"
        );
        Ok(())
    }

    // audit: COPAUA0C-098
    #[test]
    fn the_detail_key_inverts_one_clock_reading() -> TestResult {
        let world = World::load()?;
        let (xref, card) = world.first()?;
        let template = summary_template(xref);
        let lookup = found(
            xref,
            Some(world.account(xref)?),
            world.customer(xref),
            SummaryState::Created(&template),
        );
        let request = asking(card, b"100.00")?;
        let decision = decide(&request, &lookup);
        // Day 32 of 2025 at 13:45:07.123: YYDDD 25032 and HHMMSSmmm 134507123.
        let at = NaiveDate::from_yo_opt(2025, 32)
            .and_then(|day| day.and_hms_milli_opt(13, 45, 7, 123))
            .ok_or("no such instant")?;
        let detail = detail_record(&request, &decision, &FixedClock::new(at));
        assert_eq!(detail.auth_date_9c.get(), 74_967, "COPAUA0C-098");
        assert_eq!(detail.auth_time_9c.get(), 865_492_876, "COPAUA0C-098");
        // One instant gives one key, so a second detail under it takes the `I004` path.
        let again = detail_record(&request, &decision, &FixedClock::new(at));
        assert_eq!(
            (again.auth_date_9c, again.auth_time_9c),
            (detail.auth_date_9c, detail.auth_time_9c),
            "COPAUA0C-098"
        );
        // Any reading gives keys inside `S9(05)` and `S9(09)`, newer instants lower ones.
        let readings = [
            instant((2000, 1, 1), (0, 0, 0, 0))?,
            instant((2024, 12, 31), (12, 0, 0, 500))?,
            instant((2099, 12, 31), (23, 59, 59, 999))?,
        ];
        let mut previous: Option<PendingAuthDetailRecord> = None;
        for reading in readings {
            let detail = detail_record(&request, &decision, &FixedClock::new(reading));
            assert!(
                (0..=99_999).contains(&detail.auth_date_9c.get()),
                "COPAUA0C-098"
            );
            assert!(
                (0..=999_999_999).contains(&detail.auth_time_9c.get()),
                "COPAUA0C-098"
            );
            if let Some(earlier) = &previous {
                assert!(
                    detail.auth_date_9c.get() < earlier.auth_date_9c.get(),
                    "COPAUA0C-098"
                );
            }
            previous = Some(detail);
        }
        Ok(())
    }

    /// A request for the first cross-reference card with `changes` applied, decided on a
    /// summary it creates, with its fixture account and customer.
    fn decided(
        world: &World,
        changes: &[(usize, &[u8])],
    ) -> Result<(AuthRequestRecord, Decision), Box<dyn Error>> {
        let (xref, card) = world.first()?;
        let template = summary_template(xref);
        let lookup = found(
            xref,
            Some(world.account(xref)?),
            world.customer(xref),
            SummaryState::Created(&template),
        );
        let request = parse_request(&body(card, changes))?;
        let decision = decide(&request, &lookup);
        Ok((request, decision))
    }

    // audit: COPAUA0C-100
    #[test]
    fn the_detail_copies_the_request_fields_as_moved() -> TestResult {
        let world = World::load()?;
        let clock = FixedClock::new(instant((2022, 6, 18), (14, 30, 12, 0))?);
        let (request, approved) = decided(&world, &[(8, b"123456"), (12, b"07")])?;
        let detail = detail_record(&request, &approved, &clock);
        assert_eq!(detail.orig_date, request.auth_date, "COPAUA0C-100");
        assert_eq!(detail.orig_time, request.auth_time, "COPAUA0C-100");
        assert_eq!(detail.auth_type, request.auth_type, "COPAUA0C-100");
        assert_eq!(
            detail.card_expiry_date, request.card_expiry_date,
            "COPAUA0C-100"
        );
        assert_eq!(detail.message_type, request.message_type, "COPAUA0C-100");
        assert_eq!(
            detail.message_source, request.message_source,
            "COPAUA0C-100"
        );
        assert_eq!(
            detail.merchant_catagory_code, request.merchant_catagory_code,
            "COPAUA0C-100"
        );
        assert_eq!(
            detail.acqr_country_code, request.acqr_country_code,
            "COPAUA0C-100"
        );
        assert_eq!(detail.merchant_id, request.merchant_id, "COPAUA0C-100");
        assert_eq!(detail.merchant_name, request.merchant_name, "COPAUA0C-100");
        assert_eq!(detail.merchant_city, request.merchant_city, "COPAUA0C-100");
        assert_eq!(
            detail.merchant_state, request.merchant_state,
            "COPAUA0C-100"
        );
        assert_eq!(detail.merchant_zip, request.merchant_zip, "COPAUA0C-100");
        assert_eq!(
            detail.transaction_id, request.transaction_id,
            "COPAUA0C-100"
        );
        // The full card number is stored, which is storage and not egress; compared unprinted.
        assert!(detail.card_num == request.card_num, "COPAUA0C-100");
        assert_eq!(detail.processing_code.get(), 123_456, "COPAUA0C-100");
        assert_eq!(detail.pos_entry_mode.get(), 7, "COPAUA0C-100");
        assert!(
            detail.transaction_amt == request.transaction_amt,
            "COPAUA0C-100"
        );
        let (refused, declined) = decided(&world, &[(9, b"999999.00")])?;
        let declined = detail_record(&refused, &declined, &clock);
        assert!(
            declined.transaction_amt == refused.transaction_amt,
            "COPAUA0C-100"
        );
        Ok(())
    }

    // audit: COPAUA0C-101
    #[test]
    fn the_detail_carries_the_reply_and_its_match_status() -> TestResult {
        let world = World::load()?;
        let clock = FixedClock::new(instant((2022, 6, 18), (14, 30, 12, 0))?);
        let (request, approved) = decided(&world, &[])?;
        let detail = detail_record(&request, &approved, &clock);
        assert_eq!(detail.auth_id_code, request.auth_time, "COPAUA0C-101");
        assert_eq!(detail.auth_resp_code.as_bytes(), b"00", "COPAUA0C-101");
        assert_eq!(detail.auth_resp_reason.as_bytes(), b"0000", "COPAUA0C-101");
        assert!(
            detail.approved_amt == request.transaction_amt,
            "COPAUA0C-101"
        );
        assert_eq!(
            detail.match_status.as_bytes(),
            PendingAuthDetailRecord::MATCH_PENDING,
            "COPAUA0C-101"
        );
        assert_eq!(detail.match_status.as_bytes(), b"P", "COPAUA0C-101");
        assert_eq!(detail.auth_fraud.as_bytes(), b" ", "COPAUA0C-101");
        assert!(detail.fraud_rpt_date.is_spaces(), "COPAUA0C-101");
        let (refused, declined) = decided(&world, &[(9, b"999999.00")])?;
        let detail = detail_record(&refused, &declined, &clock);
        assert_eq!(detail.auth_id_code, refused.auth_time, "COPAUA0C-101");
        assert_eq!(detail.auth_resp_code.as_bytes(), b"05", "COPAUA0C-101");
        assert_eq!(detail.auth_resp_reason.as_bytes(), b"4100", "COPAUA0C-101");
        assert!(detail.approved_amt.is_zero(), "COPAUA0C-101");
        assert_eq!(detail.match_status.as_bytes(), b"D", "COPAUA0C-101");
        assert_eq!(detail.auth_fraud.as_bytes(), b" ", "COPAUA0C-101");
        assert!(detail.fraud_rpt_date.is_spaces(), "COPAUA0C-101");
        Ok(())
    }

    // audit: COPAUA0C-102
    #[test]
    fn the_detail_filler_is_blocked() -> TestResult {
        let world = World::load()?;
        let (xref, card) = world.first()?;
        let template = summary_template(xref);
        let lookup = found(
            xref,
            Some(world.account(xref)?),
            world.customer(xref),
            SummaryState::Created(&template),
        );
        let request = asking(card, b"100.00")?;
        // HALTED(H2): the 17-byte FILLER is never assigned. The detail is built and encoded for
        // coverage; the FILLER bytes are not asserted.
        let detail = detail_record(
            &request,
            &decide(&request, &lookup),
            &FixedClock::new(instant((2022, 6, 18), (0, 0, 0, 0))?),
        );
        assert_eq!(
            detail.encode().len(),
            PendingAuthDetailRecord::WIDTH,
            "CIPAUDTY"
        );
        blocked("COPAUA0C-102", "H2");
        Ok(())
    }

    /// The clock of the log-row tests.
    fn log_clock() -> Result<FixedClock, Box<dyn Error>> {
        Ok(FixedClock::new(instant((2022, 6, 18), (14, 30, 12, 345))?))
    }

    /// The location, level, subsystem and message the audit gives a log site.
    struct SiteRow<'a> {
        location: &'a [u8; 4],
        level: ErrorLevel,
        subsystem: ErrorSubsystem,
        message: &'a str,
    }

    impl<'a> SiteRow<'a> {
        /// The row as `LogSite` itself states it.
        fn of(site: LogSite<'_>) -> SiteRow<'static> {
            SiteRow {
                location: site.location(),
                level: site.level(),
                subsystem: site.subsystem(),
                message: site.message(),
            }
        }

        /// A warning of the application subsystem, the not-found sites.
        const fn warning(location: &'a [u8; 4], message: &'a str) -> Self {
            Self {
                location,
                level: ErrorLevel::Warning,
                subsystem: ErrorSubsystem::App,
                message,
            }
        }

        /// A critical row of `subsystem`, the failure sites.
        const fn critical(
            location: &'a [u8; 4],
            subsystem: ErrorSubsystem,
            message: &'a str,
        ) -> Self {
            Self {
                location,
                level: ErrorLevel::Critical,
                subsystem,
                message,
            }
        }
    }

    /// Asserts the location, level, subsystem and padded message of a log row.
    fn assert_site(row: &ErrorLogRecord, expected: &SiteRow<'_>, rule: &str) {
        assert_eq!(row.location.as_bytes(), expected.location, "{rule}");
        assert_eq!(row.level(), Some(expected.level), "{rule}");
        assert_eq!(row.subsystem(), Some(expected.subsystem), "{rule}");
        assert_eq!(
            row.message.as_bytes(),
            padded(expected.message.as_bytes(), 50).as_slice(),
            "{rule}"
        );
    }

    // audit: COPAUA0C-110
    #[test]
    fn every_log_row_is_the_122_byte_cssl_record() -> TestResult {
        let world = World::load()?;
        let (xref, card) = world.first()?;
        let card_num = &xref.card_num;
        let sqlstate = SqlState::parse("23505");
        let sites = [
            LogSite::A001 { card: card_num },
            LogSite::A002 {
                acct_id: xref.acct_id,
            },
            LogSite::A003 {
                cust_id: xref.cust_id,
            },
            LogSite::C001 {
                card: card_num,
                sqlstate,
            },
            LogSite::C002 {
                acct_id: xref.acct_id,
                sqlstate,
            },
            LogSite::C003 {
                cust_id: xref.cust_id,
                sqlstate,
            },
            LogSite::I001,
            LogSite::I002 {
                card: card_num,
                sqlstate,
            },
            LogSite::M004 { card: card_num },
            LogSite::I003 {
                card: card_num,
                sqlstate,
            },
            LogSite::I004 {
                card: card_num,
                sqlstate,
            },
        ];
        assert_eq!(ErrorLogRecord::WIDTH, 122, "COPAUA0C-110");
        assert_eq!(TRANSACTION, "CP00", "COPAUA0C-110");
        assert_eq!(PROGRAM, "COPAUA0C", "COPAUA0C-110");
        let clock = log_clock()?;
        for site in sites {
            let row = error_log(site, &clock);
            // One clock reading per row, `YYMMDD` and `HHMMSS`, compared by format only.
            assert!(has_format(row.date.as_bytes(), b"999999"), "COPAUA0C-110");
            assert!(has_format(row.time.as_bytes(), b"999999"), "COPAUA0C-110");
            assert_eq!(row.application.as_bytes(), b"CP00    ", "COPAUA0C-110");
            assert_eq!(row.program.as_bytes(), b"COPAUA0C", "COPAUA0C-110");
            assert_site(&row, &SiteRow::of(site), "COPAUA0C-110");
            assert_eq!(
                site.ends_task(),
                site.level() == ErrorLevel::Critical,
                "COPAUA0C-110"
            );
            let encoded = row.encode();
            assert_eq!(encoded.len(), ErrorLogRecord::WIDTH, "COPAUA0C-110");
            assert!(!contains(&encoded, card), "COPAUA0C-110");
        }
        Ok(())
    }

    // audit: COPAUA0C-114
    // audit: COPAUA0C-115
    // audit: COPAUA0C-116
    #[test]
    fn not_found_rows_are_application_warnings() -> TestResult {
        let world = World::load()?;
        let (xref, card) = world.first()?;
        let line = world.xref_lines.first().ok_or("no cross-reference line")?;
        let clock = log_clock()?;
        let redacted_key = padded(&[b"************", segment(card, 12, 4)?].concat(), 20);
        let cases = [
            (
                LogSite::A001 {
                    card: &xref.card_num,
                },
                SiteRow::warning(b"A001", "CARD NOT FOUND IN XREF"),
                redacted_key,
                "COPAUA0C-114",
            ),
            (
                LogSite::A002 {
                    acct_id: xref.acct_id,
                },
                SiteRow::warning(b"A002", "ACCT NOT FOUND IN XREF"),
                padded(segment(line, 25, 11)?, 20),
                "COPAUA0C-115",
            ),
            (
                LogSite::A003 {
                    cust_id: xref.cust_id,
                },
                SiteRow::warning(b"A003", "CUST NOT FOUND IN XREF"),
                padded(segment(line, 16, 9)?, 20),
                "COPAUA0C-116",
            ),
        ];
        for (site, expected, key, rule) in &cases {
            let row = error_log(*site, &clock);
            assert_site(&row, expected, rule);
            assert!(row.event_key.as_bytes() == key.as_slice(), "{rule}");
            assert!(!site.ends_task(), "{rule}");
        }
        Ok(())
    }

    // audit: COPAUA0C-117
    // audit: COPAUA0C-119
    // audit: COPAUA0C-120
    // audit: COPAUA0C-122
    // audit: COPAUA0C-123
    // audit: COPAUA0C-124
    // audit: COPAUA0C-125
    // audit: COPAUA0C-126
    #[test]
    fn failure_rows_are_critical_and_end_the_task() -> TestResult {
        let world = World::load()?;
        let (xref, _) = world.first()?;
        let card_num = &xref.card_num;
        let sqlstate = SqlState::parse("08006");
        let clock = log_clock()?;
        let (mq, ims, cics) = (
            ErrorSubsystem::Mq,
            ErrorSubsystem::Ims,
            ErrorSubsystem::Cics,
        );
        let cases = [
            (
                LogSite::M004 { card: card_num },
                SiteRow::critical(b"M004", mq, "FAILED TO PUT ON REPLY MQ"),
                "COPAUA0C-117",
            ),
            (
                LogSite::I003 {
                    card: card_num,
                    sqlstate,
                },
                SiteRow::critical(b"I003", ims, "IMS UPDATE SUMRY FAILED"),
                "COPAUA0C-119",
            ),
            (
                LogSite::I004 {
                    card: card_num,
                    sqlstate,
                },
                SiteRow::critical(b"I004", ims, "IMS INSERT DETL FAILED"),
                "COPAUA0C-120",
            ),
            (
                LogSite::C001 {
                    card: card_num,
                    sqlstate,
                },
                SiteRow::critical(b"C001", cics, "FAILED TO READ XREF FILE"),
                "COPAUA0C-122",
            ),
            (
                LogSite::C002 {
                    acct_id: xref.acct_id,
                    sqlstate,
                },
                SiteRow::critical(b"C002", cics, "FAILED TO READ ACCT FILE"),
                "COPAUA0C-123",
            ),
            (
                LogSite::C003 {
                    cust_id: xref.cust_id,
                    sqlstate,
                },
                SiteRow::critical(b"C003", cics, "FAILED TO READ CUST FILE"),
                "COPAUA0C-124",
            ),
            (
                LogSite::I002 {
                    card: card_num,
                    sqlstate,
                },
                SiteRow::critical(b"I002", ims, "IMS GET SUMMARY FAILED"),
                "COPAUA0C-125",
            ),
            (
                LogSite::I001,
                SiteRow::critical(b"I001", ims, "IMS SCHD FAILED"),
                "COPAUA0C-126",
            ),
        ];
        for (site, expected, rule) in &cases {
            assert_site(&error_log(*site, &clock), expected, rule);
            assert!(site.ends_task(), "{rule}");
        }
        Ok(())
    }

    // audit: COPAUA0C-120
    // audit: COPAUA0C-122
    // audit: COPAUA0C-123
    // audit: COPAUA0C-124
    #[test]
    fn failure_rows_carry_the_event_key_of_this_request() -> TestResult {
        let world = World::load()?;
        let (xref, card) = world.first()?;
        let line = world.xref_lines.first().ok_or("no cross-reference line")?;
        let card_num = &xref.card_num;
        let sqlstate = SqlState::parse("08006");
        let clock = log_clock()?;
        // The redacted card then four spaces (E32), and the account and customer IDs as their
        // digits, each taken from the fixture line rather than quoted.
        let keys: [(LogSite<'_>, Vec<u8>, &str); 4] = [
            (
                LogSite::I004 {
                    card: card_num,
                    sqlstate,
                },
                padded(&[b"************", segment(card, 12, 4)?].concat(), 20),
                "COPAUA0C-120",
            ),
            (
                LogSite::C001 {
                    card: card_num,
                    sqlstate,
                },
                padded(&[b"************", segment(card, 12, 4)?].concat(), 20),
                "COPAUA0C-122",
            ),
            (
                LogSite::C002 {
                    acct_id: xref.acct_id,
                    sqlstate,
                },
                padded(segment(line, 25, 11)?, 20),
                "COPAUA0C-123",
            ),
            (
                LogSite::C003 {
                    cust_id: xref.cust_id,
                    sqlstate,
                },
                padded(segment(line, 16, 9)?, 20),
                "COPAUA0C-124",
            ),
        ];
        for (site, key, rule) in keys {
            let row = error_log(site, &clock);
            assert!(row.event_key.as_bytes() == key.as_slice(), "{rule}");
        }
        Ok(())
    }

    // audit: COPAUA0C-111
    // audit: COPAUA0C-112
    // audit: COPAUA0C-113
    #[test]
    fn unassigned_codes_and_keys_of_the_log_are_blocked() -> TestResult {
        let world = World::load()?;
        let (xref, card) = world.first()?;
        let card_num = &xref.card_num;
        let sqlstate = SqlState::parse("23505");
        let clock = log_clock()?;
        // HALTED(X-COPAUA0C-H1): `ERR-CODE-1`, `ERR-CODE-2` and the `I001` event key that no
        // statement moves; HALTED(H4): the CICS, MQ and DL/I codes. The rows are built for
        // coverage; those fields are not asserted.
        for site in [
            LogSite::A001 { card: card_num },
            LogSite::I001,
            LogSite::M004 { card: card_num },
            LogSite::I004 {
                card: card_num,
                sqlstate,
            },
        ] {
            assert_eq!(
                error_log(site, &clock).encode().len(),
                ErrorLogRecord::WIDTH,
                "COPAUA0C-110"
            );
        }
        // E34: the `I002`, `M004` and `I003` keys come from a detail field not yet assigned for
        // this request, so no parity value exists. Cardholder Data Redaction still holds: the
        // key never carries the full card number.
        for site in [
            LogSite::I002 {
                card: card_num,
                sqlstate,
            },
            LogSite::M004 { card: card_num },
            LogSite::I003 {
                card: card_num,
                sqlstate,
            },
        ] {
            let row = error_log(site, &clock);
            assert!(
                !contains(row.event_key.as_bytes(), card),
                "Cardholder Data Redaction"
            );
        }
        blocked("COPAUA0C-111", "X-COPAUA0C-H1");
        blocked("COPAUA0C-112", "H4");
        blocked("COPAUA0C-113", "E34");
        Ok(())
    }

    // audit: COPAUA0C-130
    #[test]
    fn requests_decided_in_sequence_share_nothing() -> TestResult {
        let world = World::load()?;
        let (xref, card) = world.first()?;
        let template = summary_template(xref);
        let lookup = found(
            xref,
            Some(world.account(xref)?),
            world.customer(xref),
            SummaryState::Created(&template),
        );
        let approve = asking(card, b"100.00")?;
        let refuse = asking(card, b"999999.00")?;
        let unknown = parse_request(&body(UNKNOWN_CARD.as_bytes(), &[(18, b"OTHER")]))?;
        let alone = decide(&approve, &lookup);
        // A decline, a card not in the cross-reference and a field error before it change
        // nothing: each decision and reply comes from its own request and reads.
        let first = decide(&refuse, &lookup);
        let second = decide(&unknown, &Lookup::CardNotInXref);
        let third = field_error(&body(card, &[(9, b"ABC")]))?;
        let after = decide(&approve, &lookup);
        assert!(after == alone, "COPAUA0C-130");
        // The reply pointer starts at 1 for every reply: each body starts with its own card field
        // and holds only its own 63 bytes, never an earlier reply's.
        let bodies = [
            written(&first.reply)?,
            written(&second.reply)?,
            written(&field_error_reply(&third))?,
            written(&after.reply)?,
        ];
        assert!(
            bodies.last() == Some(&written(&alone.reply)?),
            "COPAUA0C-130"
        );
        assert!(bodies.first() != bodies.last(), "COPAUA0C-130");
        for bytes in &bodies {
            assert!(segment(bytes, 0, 12)? == b"************", "COPAUA0C-130");
        }
        Ok(())
    }

    // audit: COPAUA0C-131
    #[test]
    fn later_messages_of_one_cics_task_are_blocked() -> TestResult {
        let world = World::load()?;
        let (xref, card) = world.first()?;
        let template = summary_template(xref);
        let lookup = found(
            xref,
            Some(world.account(xref)?),
            world.customer(xref),
            SummaryState::Created(&template),
        );
        // HALTED(H1): a later message of one task carries the reply pointer and decision flags of
        // the messages before it. Each request here is a task's first; the replies of a
        // multi-message task have no single value and are not asserted.
        for amount in [b"100.00".as_slice(), b"999999.00", b"100.00"] {
            let decision = decide(&asking(card, amount)?, &lookup);
            assert!(
                !contains(&reply_body(&decision.reply), card),
                "Cardholder Data Redaction"
            );
        }
        blocked("COPAUA0C-131", "H1");
        Ok(())
    }
}
