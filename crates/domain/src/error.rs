//! The business error every ported rule returns.
//!
//! A COBOL program reports a rejected request by setting `WS-ERR-FLG`, moving
//! one message into `WS-MESSAGE` and placing the cursor on one screen field with
//! `MOVE -1 TO <field>L`. [`DomainError`] carries the same three facts: what
//! kind of failure it was ([`ErrorKind`]), the message text, and the BMS fields
//! the message concerns ([`FieldError`]).
//!
//! The adapters own everything beyond that:
//!
//! - `crates/api/src/error.rs` is the only place a [`ErrorKind`] becomes an
//!   HTTP status, and the only place the `{code, message, fields}` body is
//!   serialized. This module therefore holds no status number and no
//!   serialization.
//! - Authentication and authorization failures (missing token, non-admin
//!   caller) are API concerns, not kinds; their texts live in `crate::messages`.
//! - Persistence failures arrive as `crate::ports::PortError`. Each rule turns
//!   one into a [`DomainError`] carrying the message its program shows for that
//!   failure, which differs per program and per file, so no blanket conversion
//!   from `PortError` exists.
//!
//! Informational outcomes are not errors. A path that writes nothing but shows a
//! message without setting the error flag, such as the bill-payment prompt for a
//! blank confirmation, succeeds with that message; each rule module models such
//! paths as its own outcome type.
//!
//! Message texts come only from the `crate::messages` constants and composers,
//! so this module defines none.

/// The class of a failed request, one per way the COBOL programs reject one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    /// An input edit failed: a validation paragraph set `WS-ERR-FLG` because a
    /// field holds a value its edit rejects.
    Invalid,
    /// A required field is blank or low-values, for example the empty account
    /// ID that `COBIL00C` rejects before any file access.
    Empty,
    /// A keyed read found no record: CICS `NOTFND`, file status 23 or
    /// `SQLCODE` +100.
    NotFound,
    /// A write found the key already present: CICS `DUPREC` or `DUPKEY`, or
    /// `SQLCODE` -803, for example a transaction ID that already exists.
    Duplicate,
    /// The record changed between the read and the update, as detected by
    /// `COACTUPC` `9700-CHECK-CHANGE-IN-REC` and `COCRDUPC`
    /// `9300-CHECK-CHANGE-IN-REC`.
    Conflict,
    /// Sign-on was refused by `COSGN00C`: the user is not on file or the
    /// password does not match.
    SignonFailed,
    /// Any other I/O failure. The message is the one the program shows for that
    /// failure where the audit records one, else the `crate::messages`
    /// unexpected-error text, and never internal detail.
    Unexpected,
}

impl ErrorKind {
    /// The stable machine-readable code published as the `code` of the error
    /// body.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Invalid => "INVALID",
            Self::Empty => "EMPTY",
            Self::NotFound => "NOT_FOUND",
            Self::Duplicate => "DUPLICATE",
            Self::Conflict => "CONFLICT",
            Self::SignonFailed => "SIGNON_FAILED",
            Self::Unexpected => "UNEXPECTED",
        }
    }
}

/// One screen field a failure concerns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldError {
    /// The BMS symbolic field name without its `I`/`L` suffix, exactly as the
    /// validation audit's "BMS field" column records it (for example `ACCTSID`,
    /// `CONFIRM`, `USERID`). The API publishes these names as the enum values
    /// of `fields[].field`.
    pub field: &'static str,
    /// The text shown for this field.
    pub message: String,
}

/// A rejected request: its kind, the message the COBOL program shows, and the
/// fields that message concerns, in the order the program would place the
/// cursor on them.
///
/// `message` is owned because some COBOL messages are composed at run time with
/// `STRING`, such as the `CSUTLDPY` date-edit texts that name the field.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct DomainError {
    /// The failure class the API maps to a status.
    pub kind: ErrorKind,
    /// The code of `kind`, as [`ErrorKind::code`] returns it.
    pub code: &'static str,
    /// The message text, byte-identical to the audited COBOL message.
    pub message: String,
    /// The fields the message concerns; empty when it names no field.
    pub fields: Vec<FieldError>,
}

impl DomainError {
    /// An error of `kind` with `message` and no fields.
    #[must_use]
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            code: kind.code(),
            message: message.into(),
            fields: Vec::new(),
        }
    }

    /// An [`ErrorKind::Invalid`] error.
    #[must_use]
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Invalid, message)
    }

    /// An [`ErrorKind::Empty`] error.
    #[must_use]
    pub fn empty(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Empty, message)
    }

    /// An [`ErrorKind::NotFound`] error.
    #[must_use]
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::NotFound, message)
    }

    /// An [`ErrorKind::Duplicate`] error.
    #[must_use]
    pub fn duplicate(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Duplicate, message)
    }

    /// An [`ErrorKind::Conflict`] error.
    #[must_use]
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Conflict, message)
    }

    /// An [`ErrorKind::SignonFailed`] error.
    #[must_use]
    pub fn signon_failed(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::SignonFailed, message)
    }

    /// An [`ErrorKind::Unexpected`] error.
    #[must_use]
    pub fn unexpected(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Unexpected, message)
    }

    /// Adds `field` carrying the error's own message.
    ///
    /// The COBOL screen shows a single message and puts the cursor on one
    /// field, so the field entry repeats that message.
    #[must_use]
    pub fn with_field(mut self, field: &'static str) -> Self {
        let message = self.message.clone();
        self.fields.push(FieldError { field, message });
        self
    }

    /// Adds `field` with a message of its own.
    #[must_use]
    pub fn with_field_message(mut self, field: &'static str, message: impl Into<String>) -> Self {
        self.fields.push(FieldError {
            field,
            message: message.into(),
        });
        self
    }

    /// The field the cursor goes to: the first one added.
    #[must_use]
    pub fn first_field(&self) -> Option<&'static str> {
        self.fields.first().map(|entry| entry.field)
    }
}

#[cfg(test)]
mod tests {
    use super::{DomainError, ErrorKind, FieldError};

    const TEXT: &str = "sample rejection text";
    const OTHER_TEXT: &str = "sample field text";

    const KIND_CODES: [(ErrorKind, &str); 7] = [
        (ErrorKind::Invalid, "INVALID"),
        (ErrorKind::Empty, "EMPTY"),
        (ErrorKind::NotFound, "NOT_FOUND"),
        (ErrorKind::Duplicate, "DUPLICATE"),
        (ErrorKind::Conflict, "CONFLICT"),
        (ErrorKind::SignonFailed, "SIGNON_FAILED"),
        (ErrorKind::Unexpected, "UNEXPECTED"),
    ];

    #[test]
    fn every_kind_has_its_code() {
        for (kind, code) in KIND_CODES {
            assert_eq!(kind.code(), code);
        }
    }

    #[test]
    fn new_sets_code_from_kind_and_no_fields() {
        for (kind, _) in KIND_CODES {
            let error = DomainError::new(kind, TEXT);
            assert_eq!(error.kind, kind);
            assert_eq!(error.code, kind.code());
            assert_eq!(error.message, TEXT);
            assert!(error.fields.is_empty());
            assert_eq!(error.first_field(), None);
        }
    }

    type Shorthand = fn(&'static str) -> DomainError;

    #[test]
    fn each_shorthand_builds_its_kind() {
        let cases: [(Shorthand, ErrorKind); 7] = [
            (DomainError::invalid, ErrorKind::Invalid),
            (DomainError::empty, ErrorKind::Empty),
            (DomainError::not_found, ErrorKind::NotFound),
            (DomainError::duplicate, ErrorKind::Duplicate),
            (DomainError::conflict, ErrorKind::Conflict),
            (DomainError::signon_failed, ErrorKind::SignonFailed),
            (DomainError::unexpected, ErrorKind::Unexpected),
        ];
        for (build, kind) in cases {
            assert_eq!(build(TEXT), DomainError::new(kind, TEXT));
        }
    }

    #[test]
    fn with_field_copies_the_message() {
        let error = DomainError::empty(TEXT).with_field("ACTIDIN");
        assert_eq!(
            error.fields,
            vec![FieldError {
                field: "ACTIDIN",
                message: TEXT.to_owned(),
            }]
        );
        assert_eq!(error.first_field(), Some("ACTIDIN"));
    }

    #[test]
    fn with_field_message_keeps_order_and_own_text() {
        let error = DomainError::invalid(TEXT)
            .with_field("USERID")
            .with_field_message("PASSWD", OTHER_TEXT);
        assert_eq!(error.first_field(), Some("USERID"));
        assert_eq!(error.fields.len(), 2);
        assert_eq!(error.fields[0].message, TEXT);
        assert_eq!(error.fields[1].field, "PASSWD");
        assert_eq!(error.fields[1].message, OTHER_TEXT);
        assert_eq!(error.message, TEXT);
    }

    #[test]
    fn display_is_the_message_and_there_is_no_source() {
        let error = DomainError::conflict(TEXT).with_field("ACCTSID");
        assert_eq!(error.to_string(), TEXT);
        assert!(std::error::Error::source(&error).is_none());
    }
}
