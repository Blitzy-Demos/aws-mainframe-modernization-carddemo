//! Exit codes of the `batch` subcommands and the stderr lines that report them.
//!
//! A JCL step reports its outcome as a condition code; each subcommand reports it as its
//! process exit code:
//!
//! | Code | Meaning |
//! |---|---|
//! | 0 | Success |
//! | 4 | Records rejected or failed while the run went on (`RETURN-CODE` 4) |
//! | 8 | Reward-engine halt, `posttran` only |
//! | 16 | Abend, `creastmt` capacity stop or setup failure |
//! | REPRO code | The condition code of an IDCAMS REPRO step, such as `combtran` STEP10 |
//!
//! Stdout carries only the JSON telemetry lines, so COBOL `DISPLAY` output such as the run
//! counts becomes `tracing` events inside the jobs. Stderr carries only the lines
//! [`ExitStatus::render`] returns for an abend, a halt or a setup failure. The COBOL texts among
//! them arrive already rendered by the domain values that own them, whose texts live in
//! `carddemo_domain::messages`. This module composes only `ABEND {code}`, the reward-halt line,
//! the capacity-stop line and the setup prefix.

use std::fmt;
use std::io::Write;

/// Normal end: condition code 0.
pub(crate) const SUCCESS: i32 = 0;

/// Records rejected or failed while the run went on. CBTRN02C moves 4 to `RETURN-CODE` when
/// `WS-REJECT-COUNT > 0` [app/cbl/CBTRN02C.cbl:L229-231]. COBTUPDT `9999-ABEND` displays
/// `WS-RETURN-MSG`, moves 4 to `RETURN-CODE` and processing continues
/// [app/app-transaction-type-db2/cbl/COBTUPDT.cbl:L230-232].
pub(crate) const RECORDS_REJECTED: i32 = 4;

/// Reward-engine halt, `posttran` only: accrual failed for a record, whose posting was rolled
/// back. A new code with no COBOL counterpart.
pub(crate) const REWARD_HALT: i32 = 8;

/// Abend. Every path where the COBOL calls `CEE3ABD`: CBTRN02C, CBACT04C and CBTRN03C with
/// `ABCODE` 999 [app/cbl/CBTRN02C.cbl:L707-711] [app/cbl/CBACT04C.cbl:L628-632]
/// [app/cbl/CBTRN03C.cbl:L626-630], and CBSTM03A with no code [app/cbl/CBSTM03A.CBL:L921-923].
/// Also CBPAUP0C `9999-ABEND`, which moves 16 to `RETURN-CODE`
/// [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L377-383], the `creastmt` capacity stop
/// and every setup failure.
pub(crate) const ABEND: i32 = 16;

/// How a subcommand ended: its exit code ([`ExitStatus::code`]) and its stderr lines
/// ([`ExitStatus::render`]).
#[must_use = "an exit status must be reported and returned as the process exit code"]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExitStatus {
    /// Normal end with condition code 0, for every job that completes without setting a
    /// return code. `creastmt` ends here through CBSTM03A `9999-GOBACK`, where no statement
    /// sets `RETURN-CODE` (P-CBSTM03A-1264).
    Success,

    /// The program set `RETURN-CODE` and ended normally. `posttran`: the CBTRN02C end of job
    /// sets 4 when any record was rejected [app/cbl/CBTRN02C.cbl:L229-231] (P-CBTRN02C-020).
    /// `mnttrdb2`: COBTUPDT `9999-ABEND` sets 4 when any record failed
    /// [app/app-transaction-type-db2/cbl/COBTUPDT.cbl:L230-232] (P-COBTUPDT-010). Only 0 and 4
    /// occur, and `ReturnCode(0)` exits 0.
    ReturnCode(u8),

    /// `posttran` stopped because the reward engine failed while accruing for a record. That
    /// record's CBTRN02C updates and accrual were rolled back, earlier records stay committed,
    /// and nothing was added to `DALYREJS`.
    RewardHalt {
        /// Position of the record in the input file, counted from 1.
        record_number: usize,
        /// The 16-byte `DALYTRAN-ID` text of the record.
        tran_id: String,
        /// The `Display` text of the `carddemo_domain::rewards::RewardError`.
        cause: String,
    },

    /// A COBOL abend. `9999-ABEND-PROGRAM` of CBTRN02C (`posttran`), CBACT04C (`intcalc`) and
    /// CBTRN03C (`tranrept`) calls `CEE3ABD` with `ABCODE` 999 (P-CBTRN02C-021).
    /// `9999-ABEND-PROGRAM` of CBSTM03A (`creastmt`) calls `CEE3ABD` with no code
    /// (P-CBSTM03A-011). CBPAUP0C `9999-ABEND` (`cbpaup0j`) displays its text and ends with
    /// `RETURN-CODE` 16 (P-CBPAUP0C-009).
    Abend {
        /// The `DISPLAY` lines of the abending path, in order, exactly as the domain abend
        /// value renders them.
        lines: Vec<String>,
        /// The `ABCODE` passed to `CEE3ABD`, reported as `ABEND {code}`. `None` when the
        /// program passes no code (CBSTM03A) or ends without `CEE3ABD` (CBPAUP0C).
        abcode: Option<u32>,
    },

    /// `creastmt` stopped before writing any statement file because its input exceeds the
    /// CBSTM03A in-memory tables of 51 cards and 10 transactions per card, where the COBOL
    /// subscripts run past their tables and the result is undefined.
    CapacityStop {
        /// The text of the statement rule's capacity error, one entry per line.
        lines: Vec<String>,
        /// The `halt` escalation that records the input class.
        escalation_id: String,
    },

    /// The condition code of an IDCAMS REPRO step, as the parity audit records it, carrying no
    /// other meaning: 8 here is not a reward halt. `combtran` STEP10 loads the combined file
    /// with no `REPLACE` [app/jcl/COMBTRAN.jcl:L41-48], so it ends with 0 (P-COMBTRAN-002), or
    /// with 8 or 12 after duplicate keys (P-COMBTRAN-003). The REPROC backup step of TRANBKP
    /// and TRANREPT records the same kind of code, 12, for an empty live master
    /// (P-TRANBKP-002, P-TRANREPT-001, E-37).
    ReproCondition(u8),

    /// A failure with no COBOL counterpart, outside the ported program logic: telemetry,
    /// settings, the clock override, the database connection, generation and statement files.
    /// Reported as an abend.
    SetupFailure(String),
}

impl ExitStatus {
    /// The process exit code.
    #[must_use]
    pub(crate) fn code(&self) -> i32 {
        match self {
            Self::Success => SUCCESS,
            Self::ReturnCode(return_code) => {
                let code = i32::from(*return_code);
                // The code passes through unchanged. The check pins the contract that only
                // 0 and 4 arrive here, and is the one place the binary reads RECORDS_REJECTED.
                debug_assert!(
                    matches!(code, SUCCESS | RECORDS_REJECTED),
                    "RETURN-CODE {code} is not one the carried programs set"
                );
                code
            }
            Self::RewardHalt { .. } => REWARD_HALT,
            Self::Abend { .. } | Self::CapacityStop { .. } | Self::SetupFailure(_) => ABEND,
            Self::ReproCondition(condition_code) => i32::from(*condition_code),
        }
    }

    /// The stderr lines that report this status. A normal end, a return code and a REPRO
    /// condition code print nothing.
    #[must_use]
    pub(crate) fn render(&self) -> Vec<String> {
        match self {
            Self::Success | Self::ReturnCode(_) | Self::ReproCondition(_) => Vec::new(),
            Self::RewardHalt {
                record_number,
                tran_id,
                cause,
            } => vec![format!(
                "REWARD ENGINE HALT: record {record_number} TRAN-ID {tran_id}: {}",
                one_line(cause)
            )],
            Self::Abend { lines, abcode } => lines
                .iter()
                .cloned()
                .chain(abcode.map(|code| format!("ABEND {code}")))
                .collect(),
            Self::CapacityStop {
                lines,
                escalation_id,
            } => lines
                .iter()
                .cloned()
                .chain([format!("CAPACITY STOP: halt escalation {escalation_id}")])
                .collect(),
            Self::SetupFailure(message) => vec![format!("SETUP FAILURE: {}", one_line(message))],
        }
    }

    /// Writes [`ExitStatus::render`] to stderr, one line each.
    pub(crate) fn report(&self) {
        let mut stderr = std::io::stderr().lock();
        for line in self.render() {
            // stderr is the last channel left: a failed write has nowhere to be reported.
            let _ = writeln!(stderr, "{line}");
        }
    }

    /// An abend whose `DISPLAY` lines are the lines of `err`'s text, followed by `ABEND {code}`
    /// when the program passes `abcode` to `CEE3ABD`.
    pub(crate) fn abend_from(err: &impl fmt::Display, abcode: Option<u32>) -> Self {
        Self::Abend {
            lines: text_lines(err),
            abcode,
        }
    }

    /// A setup failure reporting `err`'s text.
    pub(crate) fn setup(err: impl fmt::Display) -> Self {
        Self::SetupFailure(err.to_string())
    }

    /// A `creastmt` capacity stop reporting the lines of `err`'s text and the `halt`
    /// escalation that records the input class.
    pub(crate) fn capacity_stop(err: &impl fmt::Display, escalation_id: impl Into<String>) -> Self {
        Self::CapacityStop {
            lines: text_lines(err),
            escalation_id: escalation_id.into(),
        }
    }
}

/// The lines of `text`'s display, split on `'\n'`. A final line break adds no empty line.
fn text_lines(text: &impl fmt::Display) -> Vec<String> {
    text.to_string().lines().map(str::to_owned).collect()
}

/// `text` with its line breaks replaced by spaces, so a message composed into one report line
/// stays one stderr line.
fn one_line(text: &str) -> String {
    text.replace(['\r', '\n'], " ")
}

#[cfg(test)]
mod tests {
    use std::fmt;

    use carddemo_domain::messages::{
        ABENDING_PROGRAM, CBPAUP0C_ABENDING, ERROR_OPENING_DALYTRAN, file_status_message,
    };

    use super::{ABEND, ExitStatus, RECORDS_REJECTED, REWARD_HALT, SUCCESS};

    /// A domain-style error whose text is several `DISPLAY` lines.
    struct DisplayLines(Vec<String>);

    impl fmt::Display for DisplayLines {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(&self.0.join("\n"))
        }
    }

    /// The CBTRN02C `0000-DALYTRAN-OPEN` abend displays, in order.
    fn dalytran_open_abend_lines() -> Vec<String> {
        vec![
            ERROR_OPENING_DALYTRAN.to_owned(),
            file_status_message(*b"35"),
            ABENDING_PROGRAM.to_owned(),
        ]
    }

    fn reward_halt() -> ExitStatus {
        ExitStatus::RewardHalt {
            record_number: 17,
            tran_id: "0000000000683580".to_owned(),
            cause: "reward store write failed".to_owned(),
        }
    }

    #[test]
    fn constants_are_the_documented_codes() {
        assert_eq!(
            [SUCCESS, RECORDS_REJECTED, REWARD_HALT, ABEND],
            [0, 4, 8, 16]
        );
    }

    #[test]
    fn code_maps_every_variant() {
        let cases = [
            (ExitStatus::Success, 0),
            (ExitStatus::ReturnCode(0), 0),
            (ExitStatus::ReturnCode(4), 4),
            (reward_halt(), 8),
            (ExitStatus::abend_from(&ABENDING_PROGRAM, Some(999)), 16),
            (ExitStatus::abend_from(&CBPAUP0C_ABENDING, None), 16),
            (ExitStatus::capacity_stop(&"too many cards", "E-99"), 16),
            (ExitStatus::setup("connection refused"), 16),
            (ExitStatus::ReproCondition(0), 0),
            (ExitStatus::ReproCondition(8), 8),
            (ExitStatus::ReproCondition(12), 12),
        ];
        for (status, expected) in cases {
            assert_eq!(status.code(), expected, "{status:?}");
        }
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "RETURN-CODE 16 is not one the carried programs set")]
    fn return_code_outside_the_programs_is_a_contract_breach() {
        let _ = ExitStatus::ReturnCode(16).code();
    }

    #[test]
    fn silent_statuses_render_nothing() {
        for status in [
            ExitStatus::Success,
            ExitStatus::ReturnCode(4),
            ExitStatus::ReproCondition(12),
        ] {
            assert!(status.render().is_empty(), "{status:?}");
        }
    }

    #[test]
    fn abend_with_code_ends_with_abend_line() {
        let status = ExitStatus::Abend {
            lines: dalytran_open_abend_lines(),
            abcode: Some(999),
        };
        assert_eq!(
            status.render(),
            [
                "ERROR OPENING DALYTRAN",
                "FILE STATUS IS: NNNN0035",
                "ABENDING PROGRAM",
                "ABEND 999",
            ]
        );
    }

    #[test]
    fn abend_without_code_renders_only_its_lines() {
        for lines in [
            vec![ABENDING_PROGRAM.to_owned()],
            vec![CBPAUP0C_ABENDING.to_owned()],
        ] {
            let status = ExitStatus::Abend {
                lines: lines.clone(),
                abcode: None,
            };
            assert_eq!(status.render(), lines);
        }
    }

    #[test]
    fn abend_from_splits_the_text_into_display_lines() {
        let err = DisplayLines(dalytran_open_abend_lines());
        assert_eq!(
            ExitStatus::abend_from(&err, Some(999)),
            ExitStatus::Abend {
                lines: dalytran_open_abend_lines(),
                abcode: Some(999),
            }
        );
        assert_eq!(
            ExitStatus::abend_from(&"ABENDING PROGRAM\n", None),
            ExitStatus::Abend {
                lines: vec![ABENDING_PROGRAM.to_owned()],
                abcode: None,
            }
        );
    }

    #[test]
    fn reward_halt_names_record_and_tran_id() {
        assert_eq!(
            reward_halt().render(),
            ["REWARD ENGINE HALT: record 17 TRAN-ID 0000000000683580: reward store write failed"]
        );
    }

    #[test]
    fn capacity_stop_ends_with_the_escalation() {
        let err = DisplayLines(vec!["first line".to_owned(), "second line".to_owned()]);
        let status = ExitStatus::capacity_stop(&err, "E-99");
        assert_eq!(
            status.render(),
            [
                "first line",
                "second line",
                "CAPACITY STOP: halt escalation E-99"
            ]
        );
    }

    #[test]
    fn setup_failure_is_one_prefixed_line() {
        assert_eq!(
            ExitStatus::setup("invalid CARDDEMO_CLOCK_OVERRIDE").render(),
            ["SETUP FAILURE: invalid CARDDEMO_CLOCK_OVERRIDE"]
        );
        assert_eq!(
            ExitStatus::setup("pool timed out\r\nwhile connecting").render(),
            ["SETUP FAILURE: pool timed out  while connecting"]
        );
    }
}
