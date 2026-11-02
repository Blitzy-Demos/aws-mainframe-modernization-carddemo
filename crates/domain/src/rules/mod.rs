//! The ported COBOL rules: one pure module per carried program.
//!
//! Each module ports the paragraphs of one program the replacement carries:
//! the program of a README online transaction or of a batch job run as a CLI
//! subcommand, or a program one of those `CALL`s or `LINK`s (`CSUTLDTC`,
//! `CBSTM03B`, `COPAUS2C`) or copies in as a procedure copybook (`CSUTLDPY`,
//! `CSLKPCDY`). The modules are named for their function, not their program:
//! `bill_payment` is `COBIL00C`, `posting` is `CBTRN02C`.
//!
//! These are derive-lane ports: their expected results come from interpreting
//! COBOL paragraphs, conditions and arithmetic, so a rule implements only what a
//! reviewed row of `docs/parity/parity-oracle-audit.md` (sections B to F) or
//! `docs/parity/validation-audit.md` records. A rule that depends on a case
//! `docs/parity/escalations.md` holds at status `halt` writes no code that
//! guesses the unproven result: it returns the outcome the escalation decides,
//! or the unexpected-error outcome where the escalation decides none.
//!
//! # Conventions
//!
//! - Every ported function names the COBOL paragraph it ports and the audit
//!   rows that gate it, in the form
//!   `/// <PROGRAM> <PARAGRAPH> — P-<KEY>-<nnn>`, for example
//!   `/// CBTRN02C 1500-B-LOOKUP-ACCT — P-CBTRN02C-004`, adding the
//!   `V-<KEY>-<nnn>` rows of the messages it can return.
//! - Record access goes through the traits of [`crate::ports`], taken as
//!   generic parameters and never as trait objects; a rule commits nothing.
//! - Message texts come from [`crate::messages`] only, and a rejected request
//!   is a [`DomainError`](crate::error::DomainError) carrying the audited kind,
//!   message and BMS field.
//! - Time comes from an injected [`Clock`](crate::clock::Clock), never from the
//!   system clock directly.
//!
//! # Shared helpers
//!
//! The behaviours several programs share live here, once:
//! [`version_token`], [`VersionToken`] and the [`VersionKey`] that keys it
//! (the change check of `COACTUPC` and `COCRDUPC`), [`Confirm`] (the `X(1)`
//! confirmation fields) and [`ReturnMsg`]
//! (the first-message-wins `WS-RETURN-MSG` accumulator). Every other item is
//! imported from its own module, as `rules::<module>::<item>`.

pub mod account_update;
pub mod account_view;
pub mod auth_decision;
pub mod auth_detail;
pub mod auth_purge;
pub mod auth_summary;
pub mod bill_payment;
pub mod card_list;
pub mod card_update;
pub mod card_view;
pub mod date_edit;
pub mod date_validation;
pub mod interest;
pub mod mq_account_reply;
pub mod mq_date_reply;
pub mod posting;
pub mod report_request;
pub mod signon;
pub mod statement;
pub mod tran_type_batch;
pub mod tran_type_list;
pub mod tran_type_update;
pub mod transaction_add;
pub mod transaction_list;
pub mod transaction_report;
pub mod transaction_view;
pub mod user_add;
pub mod user_delete;
pub mod user_list;
pub mod user_update;

use std::fmt;

use argon2::{Algorithm, Argon2, Params, Version};

/// The length of a rendered [`VersionToken`]: a 64-bit digest in hex.
const VERSION_TOKEN_LEN: usize = 16;

/// The Argon2id memory cost of a version digest, in KiB: 8, the least argon2
/// accepts for one lane (`Params::MIN_M_COST`). The digest's strength is the
/// secret of the [`VersionKey`], not its cost, so every cost is the minimum
/// and a token costs microseconds per `GET` and `PUT`.
const VERSION_DIGEST_M_COST: u32 = 8;

/// The Argon2id time cost of a version digest: one pass
/// (`Params::MIN_T_COST`).
const VERSION_DIGEST_T_COST: u32 = 1;

/// The Argon2id parallelism of a version digest: one lane
/// (`Params::MIN_P_COST`).
const VERSION_DIGEST_P_COST: u32 = 1;

/// The length of a version digest in bytes: the 64 bits a [`VersionToken`]
/// renders as [`VERSION_TOKEN_LEN`] hex digits.
const VERSION_DIGEST_LEN: usize = 8;

/// The Argon2 salt of every version digest: a fixed label that separates these
/// digests from any other use of the same secret. It is not secret, and it is
/// longer than the 8 bytes argon2 requires (`argon2::MIN_SALT_LEN`).
const VERSION_DIGEST_SALT: &[u8] = b"carddemo record version token";

/// The version of a record as `GET /accounts/{id}` and `GET /cards/{id}` show
/// it: 16 lower-case hex digits that the matching `PUT` echoes back.
///
/// `COACTUPC` `9700-CHECK-CHANGE-IN-REC` [app/cbl/COACTUPC.cbl:L4109-4193] and
/// `COCRDUPC` `9300-CHECK-CHANGE-IN-REC` [app/cbl/COCRDUPC.cbl:L1498-1519]
/// compare the record read for update with the copy the program fetched for
/// display and kept in its COMMAREA; a difference answers
/// `Record changed by some one else. Please review`. The stateless API keeps
/// no COMMAREA, so the GET response carries this digest of exactly the fields
/// those paragraphs compare, and the update rule recomputes it from the row it
/// reads for update (AAP 0.3.7). Equal tokens stand for "no change detected".
/// Which fields are digested, in which order and with which case folding,
/// belongs to `account_update::record_version` and
/// `card_update::record_version`; this type is only the digest.
///
/// A request without a token has no COBOL path or message: the API answers it
/// with 400 and no COBOL text (E-24), as it does a token [`VersionToken::parse`]
/// rejects.
///
/// The digest is keyed with the server's [`VersionKey`]: only the holder of
/// the key can compute a token, so a token discloses nothing about the fields
/// it digests, the card's verification code included, which no response
/// shows. The token detects concurrent change and authenticates nothing: a
/// client echoes the token it was given and cannot forge another.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct VersionToken(String);

impl VersionToken {
    /// The 16 lower-case hex digits.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The token a request echoes back, when `text` is exactly 16 characters
    /// of `0-9` and `a-f`; `None` for any other text, upper-case hex included.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let well_formed = text.len() == VERSION_TOKEN_LEN
            && text
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'));
        well_formed.then(|| Self(text.to_owned()))
    }
}

/// The 16 hex digits, as the API publishes them.
impl fmt::Display for VersionToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// The server secret that keys every [`VersionToken`].
///
/// Adapters build one once, from their server secret, and pass it to every
/// rule that issues or checks a token: the API builds it from
/// `CARDDEMO_JWT_SECRET`, the secret it already holds. Every process that
/// serves the same records must use the same secret; a token issued under
/// another one never matches, and its `PUT` answers as a changed record.
///
/// The secret is never logged: `Debug` prints `VersionKey(<redacted>)`.
#[derive(Clone)]
pub struct VersionKey {
    /// The secret bytes: the Argon2 secret value `K`.
    secret: Box<[u8]>,
    /// The Argon2id costs of every digest, checked once by [`VersionKey::new`].
    params: Params,
}

impl VersionKey {
    /// The key whose secret is `secret`.
    ///
    /// # Errors
    ///
    /// - [`VersionError::EmptyKey`] when `secret` is empty: an empty key would
    ///   leave the digest unkeyed, computable by anyone from the record.
    /// - [`VersionError::Digest`] when argon2 rejects the secret: one longer
    ///   than its 4 GiB secret limit (`argon2::MAX_SECRET_LEN`).
    pub fn new(secret: &[u8]) -> Result<Self, VersionError> {
        if secret.is_empty() {
            return Err(VersionError::EmptyKey);
        }
        let params = Params::new(
            VERSION_DIGEST_M_COST,
            VERSION_DIGEST_T_COST,
            VERSION_DIGEST_P_COST,
            Some(VERSION_DIGEST_LEN),
        )
        .map_err(digest_error)?;
        Argon2::new_with_secret(secret, Algorithm::Argon2id, Version::V0x13, params.clone())
            .map_err(digest_error)?;
        Ok(Self {
            secret: secret.into(),
            params,
        })
    }

    /// The Argon2id (version `0x13`) context keyed with this secret.
    fn argon2(&self) -> Result<Argon2<'_>, VersionError> {
        Argon2::new_with_secret(
            &self.secret,
            Algorithm::Argon2id,
            Version::V0x13,
            self.params.clone(),
        )
        .map_err(digest_error)
    }
}

/// `VersionKey(<redacted>)`, whatever the secret.
impl fmt::Debug for VersionKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("VersionKey(<redacted>)")
    }
}

/// A failure to build a [`VersionKey`] or to compute a [`VersionToken`].
///
/// No variant carries the secret or a digested field: the payload is argon2's
/// error text, which holds neither.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum VersionError {
    /// [`VersionKey::new`] was given an empty secret, which would leave every
    /// token unkeyed.
    #[error("the version key secret is empty")]
    EmptyKey,
    /// argon2 rejected the key, its costs or the digest input.
    #[error("version digest failed: {0}")]
    Digest(String),
}

/// The [`VersionError::Digest`] of an argon2 `error`.
fn digest_error(error: argon2::Error) -> VersionError {
    VersionError::Digest(error.to_string())
}

/// The [`VersionToken`] under `key` of a record whose compared fields are
/// `parts`, in the order the change check compares them.
///
/// Each part is encoded as its length, eight little-endian bytes, followed by
/// its bytes, so moving bytes from one part to the next changes the token:
/// `["ab", "c"]` and `["a", "bc"]` differ. The encoding is the Argon2id
/// (version `0x13`) password input, keyed with the secret of `key` and salted
/// with a fixed, non-secret domain-separation label, at 8 KiB, one pass and
/// one lane; the 8 output bytes, read big-endian, are the 16 hex digits.
/// Equal keys and parts always give the same token, whatever process computes
/// it. Without the key nobody can compute or check a token, so it discloses
/// nothing about `parts`.
///
/// # Errors
///
/// [`VersionError::Digest`] when argon2 rejects the input: only an encoding
/// longer than its 4 GiB input limit (`argon2::MAX_PWD_LEN`), which no
/// record's compared fields reach.
pub fn version_token(key: &VersionKey, parts: &[&[u8]]) -> Result<VersionToken, VersionError> {
    let mut encoded = Vec::new();
    for part in parts {
        // A slice longer than u64::MAX bytes cannot exist on any supported
        // target; the fallback only keeps the conversion free of a panic.
        let len = u64::try_from(part.len()).unwrap_or(u64::MAX);
        encoded.extend_from_slice(&len.to_le_bytes());
        encoded.extend_from_slice(part);
    }
    let mut tag = [0u8; VERSION_DIGEST_LEN];
    key.argon2()?
        .hash_password_into(&encoded, VERSION_DIGEST_SALT, &mut tag)
        .map_err(digest_error)?;
    Ok(VersionToken(format!("{:016x}", u64::from_be_bytes(tag))))
}

/// The class of a value entered in an `X(1)` confirmation field.
///
/// The three programs with such a field map the classes differently, so this
/// type only classifies and each rule decides:
///
/// - `COBIL00C` `PROCESS-ENTER-KEY` [app/cbl/COBIL00C.cbl:L173-191]: `Y`
///   pays, `N` clears the screen, blank shows the balance and asks for
///   confirmation, anything else is rejected as an invalid value.
/// - `COTRN02C` `PROCESS-ENTER-KEY` [app/cbl/COTRN02C.cbl:L169-188]: `Y` adds
///   the transaction, `N` and blank both ask for confirmation, anything else
///   is rejected as an invalid value.
/// - `CORPT00C` `SUBMIT-JOB-TO-INTRDR` [app/cbl/CORPT00C.cbl:L464-494]: blank
///   asks for confirmation, `Y` submits, `N` clears the screen, anything else
///   is quoted back in its message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Confirm {
    /// `Y` or `y`.
    Yes,
    /// `N` or `n`.
    No,
    /// Nothing entered: `SPACES` or `LOW-VALUES`.
    Blank,
    /// Any other text, exactly as entered. `CORPT00C` quotes it in
    /// `"<value>" is not a valid value to confirm...`, cut at the first space
    /// (`DELIMITED BY SPACE`); that message is composed in [`crate::messages`].
    Other(String),
}

impl Confirm {
    /// The class of the confirmation `input`, `None` when the field was not
    /// sent.
    ///
    /// Absent or empty text, or text of spaces only (`SPACES`) or of NUL
    /// characters only (`LOW-VALUES`), is [`Confirm::Blank`]. Exactly `Y` or
    /// `y` is [`Confirm::Yes`] and exactly `N` or `n` [`Confirm::No`]; every
    /// other text, `YY` and `Y ` included, is [`Confirm::Other`] unchanged.
    #[must_use]
    pub fn parse(input: Option<&str>) -> Self {
        match input {
            None => Self::Blank,
            Some(text) if text.bytes().all(|byte| byte == b' ') => Self::Blank,
            Some(text) if text.bytes().all(|byte| byte == 0) => Self::Blank,
            Some("Y" | "y") => Self::Yes,
            Some("N" | "n") => Self::No,
            Some(other) => Self::Other(other.to_owned()),
        }
    }
}

/// The `WS-RETURN-MSG` accumulator of the programs that run every edit and
/// report only the first failure.
///
/// `COACTUPC`, `COCRDUPC`, `COCRDLIC`, `COCRDSLC`, `COACTVWC`, `COTRTLIC`,
/// `COTRTUPC` and the `CSUTLDPY` date edits perform all their edits, and each
/// failing edit writes its message only `IF WS-RETURN-MSG-OFF`, the 88-level
/// `VALUE SPACES` of the field (for example [app/cpy/CSUTLDPY.cpy:L34-40],
/// [app/cbl/COACTUPC.cbl:L479-480]). The first message set therefore stands,
/// with the BMS field it concerns, and later failures change nothing.
///
/// The accumulator keeps the full text. Where the audit records that a
/// message is cut at the receiver's width (`WS-RETURN-MSG X(75)` in
/// `COACTUPC`), the rule cuts it with `crate::format::move_alnum`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReturnMsg {
    message: Option<String>,
    field: Option<&'static str>,
}

impl ReturnMsg {
    /// `WS-RETURN-MSG-OFF`: no message has been set yet.
    #[must_use]
    pub fn is_off(&self) -> bool {
        self.message.is_none()
    }

    /// `IF WS-RETURN-MSG-OFF STRING … INTO WS-RETURN-MSG`: records `message`
    /// and the BMS `field` it concerns when no message is set yet, and does
    /// nothing otherwise.
    ///
    /// A message of spaces only, or an empty one, leaves the accumulator off,
    /// as a `STRING` of spaces leaves `WS-RETURN-MSG` equal to its
    /// `VALUE SPACES` 88-level.
    pub fn set_if_off(&mut self, message: impl Into<String>, field: &'static str) {
        if !self.is_off() {
            return;
        }
        let message = message.into();
        if message.bytes().all(|byte| byte == b' ') {
            return;
        }
        self.message = Some(message);
        self.field = Some(field);
    }

    /// The first message set, if any.
    #[must_use]
    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    /// The BMS field of the first message set, if any.
    #[must_use]
    pub fn field(&self) -> Option<&'static str> {
        self.field
    }

    /// The first message and its BMS field, or `None` when every edit passed.
    #[must_use]
    pub fn into_parts(self) -> Option<(String, &'static str)> {
        self.message.zip(self.field)
    }
}

/// Test support for the rule modules: a runtime-free executor and an
/// in-memory store that implements every port.
///
/// The domain crate has no async runtime and no dev-dependency that provides
/// one, so the rules' unit tests drive their futures with [`block_on`] and
/// serve them from a [`FakeStore`].
#[cfg(test)]
pub(crate) mod test_support {
    use std::collections::BTreeMap;
    use std::future::Future;
    use std::ops::Bound;
    use std::pin::{Pin, pin};
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake, Waker};
    use std::thread::{self, Thread};

    use chrono::NaiveDateTime;

    use crate::entities::{
        Account, AuthFraud, Card, CardXref, Customer, DisclosureGroup, DisclosureGroupKey,
        NewPendingAuthDetail, NewTransaction, PendingAuthDetail, PendingAuthSummary,
        TranCatBalance, TranCatBalanceKey, TranCategoryMaster, TranCategoryRecord, TranTypeMaster,
        TranTypeRecord, Transaction, UserSecurity,
    };
    use crate::numeric::{
        AccountId, Alnum, AuthId, CardNumber, CustomerId, TranCategoryCode, TranId, TranTypeCode,
        TransactionRowId, UserId,
    };
    use crate::ports::{
        AccountReader, AccountWriter, AuthorizationStore, Browse, CardFilter, CardReader,
        CardWriter, CustomerReader, CustomerWriter, Direction, DisclosureGroupReader, PortError,
        PortResult, TranCatBalanceStore, TranTypeFileReader, TranTypeFilter, TranTypeMasterStore,
        TransactionReader, TransactionWriter, UserStore, XrefReader,
    };

    use super::VersionKey;

    /// The secret [`version_key`] uses in place of a server secret.
    const TEST_VERSION_SECRET: &[u8] = b"carddemo rule tests version secret";

    /// The [`VersionKey`] the rules' tests issue and check their tokens with,
    /// as an adapter's server secret keys them.
    pub(crate) fn version_key() -> VersionKey {
        VersionKey::new(TEST_VERSION_SECRET).expect("a non-empty test secret")
    }

    /// Wakes the thread [`block_on`] parks.
    struct ThreadWaker(Thread);

    impl Wake for ThreadWaker {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.0.unpark();
        }
    }

    /// Drives `fut` to completion on the current thread: polls it, and parks
    /// the thread whenever it is pending until its waker unparks it.
    pub(crate) fn block_on<F: Future>(fut: F) -> F::Output {
        let waker = Waker::from(Arc::new(ThreadWaker(thread::current())));
        let mut context = Context::from_waker(&waker);
        let mut fut = pin!(fut);
        loop {
            match fut.as_mut().poll(&mut context) {
                Poll::Ready(output) => return output,
                // A spurious unpark only costs one more poll.
                Poll::Pending => thread::park(),
            }
        }
    }

    /// Pending on its first poll, after waking its task, and ready on the
    /// next: each [`FakeStore`] call crosses one await point holding
    /// `&mut self`, as an adapter's query does.
    struct YieldOnce(bool);

    impl Future for YieldOnce {
        type Output = ();

        fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<()> {
            if self.0 {
                Poll::Ready(())
            } else {
                self.0 = true;
                context.waker().wake_by_ref();
                Poll::Pending
            }
        }
    }

    /// The name of every port method, in the order `crate::ports` declares
    /// them: the names [`FakeStore::fail_next`] accepts and
    /// [`FakeStore::calls`] records.
    pub(crate) const PORT_METHODS: [&str; 54] = [
        "read_account",
        "read_account_for_update",
        "rewrite_account",
        "read_card",
        "read_card_for_update",
        "browse_cards",
        "cards_for_account",
        "rewrite_card",
        "read_customer",
        "read_customer_for_update",
        "rewrite_customer",
        "read_xref_by_card",
        "read_xref_by_account",
        "xrefs_after",
        "read_transaction",
        "browse_transactions",
        "highest_transaction",
        "transactions_by_card",
        "transactions_by_proc_date",
        "write_transaction",
        "read_tcatbal",
        "write_tcatbal",
        "rewrite_tcatbal",
        "tcatbals_after",
        "read_disclosure_group",
        "read_tran_type",
        "browse_tran_types",
        "count_tran_types",
        "insert_tran_type",
        "update_tran_type",
        "delete_tran_type",
        "read_tran_type_file",
        "read_tran_category_file",
        "read_user",
        "read_user_for_update",
        "browse_users",
        "insert_user",
        "update_user",
        "delete_user",
        "read_summary",
        "read_summary_of_detail",
        "insert_summary",
        "update_summary",
        "delete_summary",
        "summaries_after",
        "insert_detail",
        "read_detail",
        "read_detail_for_update",
        "details_for_account",
        "adjacent_detail",
        "update_detail",
        "delete_detail",
        "insert_fraud",
        "update_fraud",
    ];

    /// An in-memory store behind every port of [`crate::ports`], with the
    /// outcomes the port documentation defines.
    ///
    /// Each file or table is a map keyed by its record key, so reads, browses
    /// and sweeps see the rows in COBOL key order. The alternate paths
    /// (`CARDAIX` cards by account, `CXACAIX` cross-references by account) are
    /// answered from the base maps in base-key order, so they cannot drift
    /// from them. The details of one pending-authorization summary are twins
    /// in `PAUT9CTS` order: ascending (`pa_auth_date_9c`, `pa_auth_time_9c`).
    ///
    /// Every call is appended to [`FakeStore::calls`] by its method name, and
    /// first consumes a failure queued for that method by
    /// [`FakeStore::fail_next`]: a failed call changes nothing, as a write
    /// rolled back to its savepoint does, and the writes before it stay in
    /// place.
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub(crate) struct FakeStore {
        /// `ACCTDAT` by `ACCT-ID`.
        pub(crate) accounts: BTreeMap<AccountId, Account>,
        /// `CARDDAT` by `CARD-NUM`.
        pub(crate) cards: BTreeMap<CardNumber, Card>,
        /// `CUSTDAT` by `CUST-ID`.
        pub(crate) customers: BTreeMap<CustomerId, Customer>,
        /// `CCXREF` by `XREF-CARD-NUM`.
        pub(crate) xrefs: BTreeMap<CardNumber, CardXref>,
        /// The live `TRANSACT` master by TRAN-ID.
        pub(crate) transactions: BTreeMap<TranId, Transaction>,
        /// `TCATBALF` by account, type code and category code.
        pub(crate) tcatbals: BTreeMap<TranCatBalanceKey, TranCatBalance>,
        /// `DISCGRP` by group, type code and category code.
        pub(crate) disclosure_groups: BTreeMap<DisclosureGroupKey, DisclosureGroup>,
        /// The Db2 master `TRANSACTION_TYPE` by `TR_TYPE`.
        pub(crate) tran_types: BTreeMap<TranTypeCode, TranTypeMaster>,
        /// The Db2 master `TRANSACTION_TYPE_CATEGORY` by type and category,
        /// whose rows make a delete of their type [`PortError::Restricted`].
        pub(crate) tran_categories: BTreeMap<(TranTypeCode, TranCategoryCode), TranCategoryMaster>,
        /// The processing copy `TRANTYPE` by type code.
        pub(crate) tran_type_file: BTreeMap<TranTypeCode, TranTypeRecord>,
        /// The processing copy `TRANCATG` by type and category.
        pub(crate) tran_category_file:
            BTreeMap<(TranTypeCode, TranCategoryCode), TranCategoryRecord>,
        /// `USRSEC` by `SEC-USR-ID`.
        pub(crate) users: BTreeMap<UserId, UserSecurity>,
        /// `PAUTSUM0` by `ACCNTID`.
        pub(crate) summaries: BTreeMap<AccountId, PendingAuthSummary>,
        /// `PAUTDTL1` by row identity.
        pub(crate) details: BTreeMap<AuthId, PendingAuthDetail>,
        /// `AUTHFRDS` by `CARD_NUM` and `AUTH_TS`.
        pub(crate) frauds: BTreeMap<(CardNumber, NaiveDateTime), AuthFraud>,
        /// The port methods called, in call order, failed calls included.
        pub(crate) calls: Vec<&'static str>,
        /// Failures queued by [`FakeStore::fail_next`], consumed in order.
        failures: Vec<(&'static str, PortError)>,
        /// The last transaction row identity assigned; the first is 1.
        last_transaction_row: i64,
        /// The last detail identity assigned; the first is 1.
        last_auth_id: i64,
    }

    /// Where a backward browse starts relative to its start key.
    #[derive(Clone, Copy)]
    enum BackwardStart {
        /// At the start key itself: `READPREV` after `STARTBR`.
        Including,
        /// Below it: the `COTRTLIC` cursor `TR_TYPE < :start`.
        Below,
    }

    /// One [`Browse`] page of `map`: keys from the start key in the browse
    /// direction, the rows `keep` rejects skipped and not counted.
    fn page<K: Ord, V: Clone>(
        map: &BTreeMap<K, V>,
        browse: &Browse<K>,
        backward_start: BackwardStart,
        keep: impl Fn(&V) -> bool,
    ) -> Vec<V> {
        let limit = usize::from(browse.limit);
        match browse.direction {
            Direction::Forward => {
                let lower = browse
                    .start_key
                    .as_ref()
                    .map_or(Bound::Unbounded, Bound::Included);
                map.range((lower, Bound::Unbounded))
                    .map(|(_, row)| row)
                    .filter(|row| keep(row))
                    .take(limit)
                    .cloned()
                    .collect()
            }
            Direction::Backward => {
                let upper = match (&browse.start_key, backward_start) {
                    (None, _) => Bound::Unbounded,
                    (Some(key), BackwardStart::Including) => Bound::Included(key),
                    (Some(key), BackwardStart::Below) => Bound::Excluded(key),
                };
                map.range((Bound::Unbounded, upper))
                    .rev()
                    .map(|(_, row)| row)
                    .filter(|row| keep(row))
                    .take(limit)
                    .cloned()
                    .collect()
            }
        }
    }

    /// Up to `limit` rows of `map` whose key is greater than `after`, in key
    /// order: one step of a batch sweep.
    fn sweep<K: Ord, V: Clone>(map: &BTreeMap<K, V>, after: Option<&K>, limit: u16) -> Vec<V> {
        let lower = after.map_or(Bound::Unbounded, Bound::Excluded);
        map.range((lower, Bound::Unbounded))
            .map(|(_, row)| row)
            .take(usize::from(limit))
            .cloned()
            .collect()
    }

    /// The stored row, or [`PortError::NotFound`].
    fn found<V: Clone>(row: Option<&V>) -> PortResult<V> {
        row.cloned().ok_or(PortError::NotFound)
    }

    /// Replaces the stored row `key` names with `row`, or
    /// [`PortError::NotFound`] when there is none.
    fn replace<K: Ord, V>(map: &mut BTreeMap<K, V>, key: &K, row: V) -> PortResult<()> {
        let stored = map.get_mut(key).ok_or(PortError::NotFound)?;
        *stored = row;
        Ok(())
    }

    /// Stores `row` under the new key `key`, or [`PortError::Duplicate`] when
    /// the key is taken.
    fn insert_new<K: Ord, V>(map: &mut BTreeMap<K, V>, key: K, row: V) -> PortResult<()> {
        if map.contains_key(&key) {
            return Err(PortError::Duplicate);
        }
        map.insert(key, row);
        Ok(())
    }

    /// The Db2 `LIKE` of `text` against `pattern`: `%` matches any run of
    /// characters, `_` exactly one, every other character itself, case
    /// included. There is no escape character.
    fn like(text: &[char], pattern: &[char]) -> bool {
        match pattern.split_first() {
            None => text.is_empty(),
            Some(('%', rest)) => (0..=text.len()).any(|skip| like(&text[skip..], rest)),
            Some(('_', rest)) => text.split_first().is_some_and(|(_, tail)| like(tail, rest)),
            Some((expected, rest)) => text
                .split_first()
                .is_some_and(|(head, tail)| head == expected && like(tail, rest)),
        }
    }

    /// Whether the type `row` passes the `COTRTLIC` filters.
    fn passes(filter: &TranTypeFilter, row: &TranTypeMaster) -> bool {
        let type_matches = filter.type_code.is_none_or(|code| row.tr_type == code);
        let description_matches = filter.description.as_ref().is_none_or(|pattern| {
            let text: Vec<char> = row.tr_description.chars().collect();
            let pattern: Vec<char> = pattern.chars().collect();
            like(&text, &pattern)
        });
        type_matches && description_matches
    }

    /// The IMS twin key of a detail: its `PAUT9CTS` sequence field, with the
    /// row identity breaking ties the database would refuse.
    fn twin_key(detail: &PendingAuthDetail) -> (i32, i32, AuthId) {
        (
            detail.pa_auth_date_9c,
            detail.pa_auth_time_9c,
            detail.auth_id,
        )
    }

    impl FakeStore {
        /// Queues `error` as the outcome of the next call of the port method
        /// named `method`. Failures queued for one method are consumed in the
        /// order queued; the failed call changes nothing.
        ///
        /// # Panics
        ///
        /// When `method` names no port method, so a misspelt name fails the
        /// test instead of never firing.
        pub(crate) fn fail_next(&mut self, method: &'static str, error: PortError) {
            assert!(
                PORT_METHODS.contains(&method),
                "no port method is named {method:?}"
            );
            self.failures.push((method, error));
        }

        /// Records the call of `method`, waits once as a query does, and
        /// returns the failure queued for it, if any.
        async fn enter(&mut self, method: &'static str) -> PortResult<()> {
            self.calls.push(method);
            YieldOnce(false).await;
            match self.failures.iter().position(|(name, _)| *name == method) {
                Some(index) => Err(self.failures.remove(index).1),
                None => Ok(()),
            }
        }

        /// Stores `tran` in the live master under the next row identity, which
        /// it returns.
        fn store_transaction(&mut self, tran: NewTransaction) -> TransactionRowId {
            self.last_transaction_row += 1;
            let row_id = TransactionRowId::from(self.last_transaction_row);
            self.transactions
                .insert(tran.tran_id, Transaction::from_new(row_id, tran));
            row_id
        }

        /// Stores `detail` under the next detail identity, which it returns.
        fn store_detail(&mut self, detail: NewPendingAuthDetail) -> AuthId {
            self.last_auth_id += 1;
            let auth_id = AuthId::from(self.last_auth_id);
            self.details
                .insert(auth_id, PendingAuthDetail::from_new(auth_id, detail));
            auth_id
        }

        /// The details of account `id` in twin order.
        fn twins(&self, id: AccountId) -> Vec<&PendingAuthDetail> {
            let mut twins: Vec<&PendingAuthDetail> = self
                .details
                .values()
                .filter(|detail| detail.pa_acct_id == id)
                .collect();
            twins.sort_by_key(|detail| twin_key(detail));
            twins
        }

        /// With `account` in `ACCTDAT`.
        #[must_use]
        pub(crate) fn with_account(mut self, account: Account) -> Self {
            self.accounts.insert(account.acct_id, account);
            self
        }

        /// With `card` in `CARDDAT`.
        #[must_use]
        pub(crate) fn with_card(mut self, card: Card) -> Self {
            self.cards.insert(card.card_num, card);
            self
        }

        /// With `customer` in `CUSTDAT`.
        #[must_use]
        pub(crate) fn with_customer(mut self, customer: Customer) -> Self {
            self.customers.insert(customer.cust_id, customer);
            self
        }

        /// With `xref` in `CCXREF`, and so on both of its paths.
        #[must_use]
        pub(crate) fn with_xref(mut self, xref: CardXref) -> Self {
            self.xrefs.insert(xref.xref_card_num, xref);
            self
        }

        /// With `tran` in the live transaction master. Row identities are
        /// assigned from 1 in the order transactions are stored.
        #[must_use]
        pub(crate) fn with_transaction(mut self, tran: NewTransaction) -> Self {
            self.store_transaction(tran);
            self
        }

        /// With `balance` in `TCATBALF`.
        #[must_use]
        pub(crate) fn with_tcatbal(mut self, balance: TranCatBalance) -> Self {
            self.tcatbals.insert(balance.key.clone(), balance);
            self
        }

        /// With `group` in `DISCGRP`.
        #[must_use]
        pub(crate) fn with_disclosure_group(mut self, group: DisclosureGroup) -> Self {
            self.disclosure_groups.insert(group.key.clone(), group);
            self
        }

        /// With `tran_type` in the Db2 type master.
        #[must_use]
        pub(crate) fn with_tran_type(mut self, tran_type: TranTypeMaster) -> Self {
            self.tran_types.insert(tran_type.tr_type, tran_type);
            self
        }

        /// With `category` in the Db2 category master.
        #[must_use]
        pub(crate) fn with_tran_category(mut self, category: TranCategoryMaster) -> Self {
            self.tran_categories.insert(
                (category.trc_type_code, category.trc_type_category),
                category,
            );
            self
        }

        /// With `record` in the processing copy `TRANTYPE`.
        #[must_use]
        pub(crate) fn with_tran_type_record(mut self, record: TranTypeRecord) -> Self {
            self.tran_type_file.insert(record.tran_type, record);
            self
        }

        /// With `record` in the processing copy `TRANCATG`.
        #[must_use]
        pub(crate) fn with_tran_category_record(mut self, record: TranCategoryRecord) -> Self {
            self.tran_category_file
                .insert((record.tran_type_cd, record.tran_cat_cd), record);
            self
        }

        /// With `user` in `USRSEC`.
        #[must_use]
        pub(crate) fn with_user(mut self, user: UserSecurity) -> Self {
            self.users.insert(user.sec_usr_id, user);
            self
        }

        /// With `summary` in `PAUTSUM0`.
        #[must_use]
        pub(crate) fn with_summary(mut self, summary: PendingAuthSummary) -> Self {
            self.summaries.insert(summary.pa_acct_id, summary);
            self
        }

        /// With `detail` in `PAUTDTL1`. Detail identities are assigned from 1
        /// in the order details are stored, whether by this builder or by
        /// `insert_detail`. Unlike `insert_detail`, the builder does not
        /// require the parent summary, so a test can seed an orphan.
        #[must_use]
        pub(crate) fn with_detail(mut self, detail: NewPendingAuthDetail) -> Self {
            self.store_detail(detail);
            self
        }

        /// With `fraud` in `AUTHFRDS`.
        #[must_use]
        pub(crate) fn with_fraud(mut self, fraud: AuthFraud) -> Self {
            self.frauds.insert((fraud.card_num, fraud.auth_ts), fraud);
            self
        }
    }

    impl AccountReader for FakeStore {
        async fn read_account(&mut self, id: AccountId) -> PortResult<Account> {
            self.enter("read_account").await?;
            found(self.accounts.get(&id))
        }

        async fn read_account_for_update(&mut self, id: AccountId) -> PortResult<Account> {
            self.enter("read_account_for_update").await?;
            found(self.accounts.get(&id))
        }
    }

    impl AccountWriter for FakeStore {
        async fn rewrite_account(&mut self, account: Account) -> PortResult<()> {
            self.enter("rewrite_account").await?;
            let key = account.acct_id;
            replace(&mut self.accounts, &key, account)
        }
    }

    impl CardReader for FakeStore {
        async fn read_card(&mut self, card: CardNumber) -> PortResult<Card> {
            self.enter("read_card").await?;
            found(self.cards.get(&card))
        }

        async fn read_card_for_update(&mut self, card: CardNumber) -> PortResult<Card> {
            self.enter("read_card_for_update").await?;
            found(self.cards.get(&card))
        }

        async fn browse_cards(
            &mut self,
            browse: Browse<CardNumber>,
            filter: CardFilter,
        ) -> PortResult<Vec<Card>> {
            self.enter("browse_cards").await?;
            Ok(page(
                &self.cards,
                &browse,
                BackwardStart::Including,
                |card| {
                    filter.account_id.is_none_or(|id| card.card_acct_id == id)
                        && filter
                            .card_number
                            .is_none_or(|number| card.card_num == number)
                },
            ))
        }

        async fn cards_for_account(&mut self, id: AccountId) -> PortResult<Vec<Card>> {
            self.enter("cards_for_account").await?;
            Ok(self
                .cards
                .values()
                .filter(|card| card.card_acct_id == id)
                .cloned()
                .collect())
        }
    }

    impl CardWriter for FakeStore {
        async fn rewrite_card(&mut self, card: Card) -> PortResult<()> {
            self.enter("rewrite_card").await?;
            let key = card.card_num;
            replace(&mut self.cards, &key, card)
        }
    }

    impl CustomerReader for FakeStore {
        async fn read_customer(&mut self, id: CustomerId) -> PortResult<Customer> {
            self.enter("read_customer").await?;
            found(self.customers.get(&id))
        }

        async fn read_customer_for_update(&mut self, id: CustomerId) -> PortResult<Customer> {
            self.enter("read_customer_for_update").await?;
            found(self.customers.get(&id))
        }
    }

    impl CustomerWriter for FakeStore {
        async fn rewrite_customer(&mut self, customer: Customer) -> PortResult<()> {
            self.enter("rewrite_customer").await?;
            let key = customer.cust_id;
            replace(&mut self.customers, &key, customer)
        }
    }

    impl XrefReader for FakeStore {
        async fn read_xref_by_card(&mut self, card: CardNumber) -> PortResult<CardXref> {
            self.enter("read_xref_by_card").await?;
            found(self.xrefs.get(&card))
        }

        async fn read_xref_by_account(&mut self, id: AccountId) -> PortResult<CardXref> {
            self.enter("read_xref_by_account").await?;
            found(self.xrefs.values().find(|xref| xref.xref_acct_id == id))
        }

        async fn xrefs_after(
            &mut self,
            after: Option<CardNumber>,
            limit: u16,
        ) -> PortResult<Vec<CardXref>> {
            self.enter("xrefs_after").await?;
            Ok(sweep(&self.xrefs, after.as_ref(), limit))
        }
    }

    impl TransactionReader for FakeStore {
        async fn read_transaction(&mut self, id: TranId) -> PortResult<Transaction> {
            self.enter("read_transaction").await?;
            found(self.transactions.get(&id))
        }

        async fn browse_transactions(
            &mut self,
            browse: Browse<TranId>,
            account_id: Option<AccountId>,
        ) -> PortResult<Vec<Transaction>> {
            self.enter("browse_transactions").await?;
            let xrefs = &self.xrefs;
            Ok(page(
                &self.transactions,
                &browse,
                BackwardStart::Including,
                |tran| {
                    account_id.is_none_or(|id| {
                        xrefs
                            .get(&tran.tran_card_num)
                            .is_some_and(|xref| xref.xref_acct_id == id)
                    })
                },
            ))
        }

        async fn highest_transaction(&mut self) -> PortResult<Option<Transaction>> {
            self.enter("highest_transaction").await?;
            Ok(self.transactions.values().next_back().cloned())
        }

        async fn transactions_by_card(&mut self, limit: u16) -> PortResult<Vec<Transaction>> {
            self.enter("transactions_by_card").await?;
            let mut rows: Vec<Transaction> = self.transactions.values().cloned().collect();
            rows.sort_by_key(|tran| (tran.tran_card_num, tran.tran_id));
            rows.truncate(usize::from(limit));
            Ok(rows)
        }

        async fn transactions_by_proc_date(
            &mut self,
            start: Alnum<10>,
            end: Alnum<10>,
            after: Option<(CardNumber, TranId)>,
            limit: u16,
        ) -> PortResult<Vec<Transaction>> {
            self.enter("transactions_by_proc_date").await?;
            let mut rows: Vec<Transaction> = self
                .transactions
                .values()
                .filter(|tran| {
                    let date = tran.tran_proc_ts.prefix(10);
                    date >= start.as_bytes().as_slice() && date <= end.as_bytes().as_slice()
                })
                .filter(|tran| after.is_none_or(|key| (tran.tran_card_num, tran.tran_id) > key))
                .cloned()
                .collect();
            rows.sort_by_key(|tran| (tran.tran_card_num, tran.tran_id));
            rows.truncate(usize::from(limit));
            Ok(rows)
        }
    }

    impl TransactionWriter for FakeStore {
        async fn write_transaction(
            &mut self,
            tran: NewTransaction,
        ) -> PortResult<TransactionRowId> {
            self.enter("write_transaction").await?;
            if self.transactions.contains_key(&tran.tran_id) {
                return Err(PortError::Duplicate);
            }
            Ok(self.store_transaction(tran))
        }
    }

    impl TranCatBalanceStore for FakeStore {
        async fn read_tcatbal(&mut self, key: TranCatBalanceKey) -> PortResult<TranCatBalance> {
            self.enter("read_tcatbal").await?;
            found(self.tcatbals.get(&key))
        }

        async fn write_tcatbal(&mut self, rec: TranCatBalance) -> PortResult<()> {
            self.enter("write_tcatbal").await?;
            insert_new(&mut self.tcatbals, rec.key.clone(), rec)
        }

        async fn rewrite_tcatbal(&mut self, rec: TranCatBalance) -> PortResult<()> {
            self.enter("rewrite_tcatbal").await?;
            let key = rec.key.clone();
            replace(&mut self.tcatbals, &key, rec)
        }

        async fn tcatbals_after(
            &mut self,
            after: Option<TranCatBalanceKey>,
            limit: u16,
        ) -> PortResult<Vec<TranCatBalance>> {
            self.enter("tcatbals_after").await?;
            Ok(sweep(&self.tcatbals, after.as_ref(), limit))
        }
    }

    impl DisclosureGroupReader for FakeStore {
        async fn read_disclosure_group(
            &mut self,
            key: DisclosureGroupKey,
        ) -> PortResult<DisclosureGroup> {
            self.enter("read_disclosure_group").await?;
            found(self.disclosure_groups.get(&key))
        }
    }

    impl TranTypeMasterStore for FakeStore {
        async fn read_tran_type(&mut self, code: TranTypeCode) -> PortResult<TranTypeMaster> {
            self.enter("read_tran_type").await?;
            found(self.tran_types.get(&code))
        }

        async fn browse_tran_types(
            &mut self,
            browse: Browse<TranTypeCode>,
            filter: TranTypeFilter,
        ) -> PortResult<Vec<TranTypeMaster>> {
            self.enter("browse_tran_types").await?;
            Ok(page(
                &self.tran_types,
                &browse,
                BackwardStart::Below,
                |row| passes(&filter, row),
            ))
        }

        async fn count_tran_types(&mut self, filter: TranTypeFilter) -> PortResult<u64> {
            self.enter("count_tran_types").await?;
            let count = self
                .tran_types
                .values()
                .filter(|row| passes(&filter, row))
                .count();
            Ok(u64::try_from(count).unwrap_or(u64::MAX))
        }

        async fn insert_tran_type(&mut self, rec: TranTypeMaster) -> PortResult<()> {
            self.enter("insert_tran_type").await?;
            insert_new(&mut self.tran_types, rec.tr_type, rec)
        }

        async fn update_tran_type(&mut self, rec: TranTypeMaster) -> PortResult<()> {
            self.enter("update_tran_type").await?;
            let key = rec.tr_type;
            replace(&mut self.tran_types, &key, rec)
        }

        async fn delete_tran_type(&mut self, code: TranTypeCode) -> PortResult<()> {
            self.enter("delete_tran_type").await?;
            if !self.tran_types.contains_key(&code) {
                return Err(PortError::NotFound);
            }
            if self
                .tran_categories
                .keys()
                .any(|(type_code, _)| *type_code == code)
            {
                return Err(PortError::Restricted);
            }
            self.tran_types.remove(&code);
            Ok(())
        }
    }

    impl TranTypeFileReader for FakeStore {
        async fn read_tran_type_file(&mut self, code: TranTypeCode) -> PortResult<TranTypeRecord> {
            self.enter("read_tran_type_file").await?;
            found(self.tran_type_file.get(&code))
        }

        async fn read_tran_category_file(
            &mut self,
            code: TranTypeCode,
            category: TranCategoryCode,
        ) -> PortResult<TranCategoryRecord> {
            self.enter("read_tran_category_file").await?;
            found(self.tran_category_file.get(&(code, category)))
        }
    }

    impl UserStore for FakeStore {
        async fn read_user(&mut self, id: UserId) -> PortResult<UserSecurity> {
            self.enter("read_user").await?;
            found(self.users.get(&id))
        }

        async fn read_user_for_update(&mut self, id: UserId) -> PortResult<UserSecurity> {
            self.enter("read_user_for_update").await?;
            found(self.users.get(&id))
        }

        async fn browse_users(&mut self, browse: Browse<UserId>) -> PortResult<Vec<UserSecurity>> {
            self.enter("browse_users").await?;
            Ok(page(&self.users, &browse, BackwardStart::Including, |_| {
                true
            }))
        }

        async fn insert_user(&mut self, user: UserSecurity) -> PortResult<()> {
            self.enter("insert_user").await?;
            insert_new(&mut self.users, user.sec_usr_id, user)
        }

        async fn update_user(&mut self, user: UserSecurity) -> PortResult<()> {
            self.enter("update_user").await?;
            let key = user.sec_usr_id;
            replace(&mut self.users, &key, user)
        }

        async fn delete_user(&mut self, id: UserId) -> PortResult<()> {
            self.enter("delete_user").await?;
            self.users.remove(&id).ok_or(PortError::NotFound)?;
            Ok(())
        }
    }

    impl AuthorizationStore for FakeStore {
        async fn read_summary(&mut self, id: AccountId) -> PortResult<PendingAuthSummary> {
            self.enter("read_summary").await?;
            found(self.summaries.get(&id))
        }

        async fn read_summary_of_detail(&mut self, id: AuthId) -> PortResult<PendingAuthSummary> {
            self.enter("read_summary_of_detail").await?;
            let account = self.details.get(&id).ok_or(PortError::NotFound)?.pa_acct_id;
            found(self.summaries.get(&account))
        }

        async fn insert_summary(&mut self, summary: PendingAuthSummary) -> PortResult<()> {
            self.enter("insert_summary").await?;
            insert_new(&mut self.summaries, summary.pa_acct_id, summary)
        }

        async fn update_summary(&mut self, summary: PendingAuthSummary) -> PortResult<()> {
            self.enter("update_summary").await?;
            let key = summary.pa_acct_id;
            replace(&mut self.summaries, &key, summary)
        }

        async fn delete_summary(&mut self, id: AccountId) -> PortResult<()> {
            self.enter("delete_summary").await?;
            self.summaries.remove(&id).ok_or(PortError::NotFound)?;
            self.details.retain(|_, detail| detail.pa_acct_id != id);
            Ok(())
        }

        async fn summaries_after(
            &mut self,
            after: Option<AccountId>,
            limit: u16,
        ) -> PortResult<Vec<PendingAuthSummary>> {
            self.enter("summaries_after").await?;
            Ok(sweep(&self.summaries, after.as_ref(), limit))
        }

        async fn insert_detail(&mut self, detail: NewPendingAuthDetail) -> PortResult<AuthId> {
            self.enter("insert_detail").await?;
            if !self.summaries.contains_key(&detail.pa_acct_id) {
                return Err(PortError::NotFound);
            }
            let same_key = self.details.values().any(|twin| {
                twin.pa_acct_id == detail.pa_acct_id
                    && twin.pa_auth_date_9c == detail.pa_auth_date_9c
                    && twin.pa_auth_time_9c == detail.pa_auth_time_9c
            });
            if same_key {
                return Err(PortError::Duplicate);
            }
            Ok(self.store_detail(detail))
        }

        async fn read_detail(&mut self, id: AuthId) -> PortResult<PendingAuthDetail> {
            self.enter("read_detail").await?;
            found(self.details.get(&id))
        }

        async fn read_detail_for_update(&mut self, id: AuthId) -> PortResult<PendingAuthDetail> {
            self.enter("read_detail_for_update").await?;
            found(self.details.get(&id))
        }

        async fn details_for_account(
            &mut self,
            id: AccountId,
            browse: Browse<AuthId>,
        ) -> PortResult<Vec<PendingAuthDetail>> {
            self.enter("details_for_account").await?;
            let mut twins = self.twins(id);
            if browse.direction == Direction::Backward {
                twins.reverse();
            }
            let start = match browse.start_key {
                None => 0,
                Some(start_key) => {
                    match twins.iter().position(|detail| detail.auth_id == start_key) {
                        Some(index) => index,
                        None => return Ok(Vec::new()),
                    }
                }
            };
            Ok(twins
                .into_iter()
                .skip(start)
                .take(usize::from(browse.limit))
                .cloned()
                .collect())
        }

        async fn adjacent_detail(
            &mut self,
            account: AccountId,
            auth_date_9c: i32,
            auth_time_9c: i32,
            direction: Direction,
        ) -> PortResult<Option<PendingAuthDetail>> {
            self.enter("adjacent_detail").await?;
            let key = (auth_date_9c, auth_time_9c);
            let twin_of =
                |detail: &&PendingAuthDetail| (detail.pa_auth_date_9c, detail.pa_auth_time_9c);
            let twins = self.twins(account);
            let adjacent = match direction {
                Direction::Forward => twins.into_iter().find(|detail| twin_of(detail) > key),
                Direction::Backward => twins.into_iter().rev().find(|detail| twin_of(detail) < key),
            };
            Ok(adjacent.cloned())
        }

        async fn update_detail(&mut self, detail: PendingAuthDetail) -> PortResult<()> {
            self.enter("update_detail").await?;
            let key = detail.auth_id;
            replace(&mut self.details, &key, detail)
        }

        async fn delete_detail(&mut self, id: AuthId) -> PortResult<()> {
            self.enter("delete_detail").await?;
            self.details.remove(&id).ok_or(PortError::NotFound)?;
            Ok(())
        }

        async fn insert_fraud(&mut self, fraud: AuthFraud) -> PortResult<()> {
            self.enter("insert_fraud").await?;
            insert_new(&mut self.frauds, (fraud.card_num, fraud.auth_ts), fraud)
        }

        async fn update_fraud(&mut self, fraud: AuthFraud) -> PortResult<()> {
            self.enter("update_fraud").await?;
            let stored = self
                .frauds
                .get_mut(&(fraud.card_num, fraud.auth_ts))
                .ok_or(PortError::NotFound)?;
            stored.auth_fraud = fraud.auth_fraud;
            stored.fraud_rpt_date = fraud.fraud_rpt_date;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::future::{Future, ready};
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::{Context, Poll};
    use std::thread;
    use std::time::Duration;

    use chrono::{NaiveDate, NaiveDateTime};

    use argon2::{Algorithm, Argon2, Params, Version};

    use super::test_support::{FakeStore, PORT_METHODS, block_on, version_key};
    use super::{
        Confirm, ReturnMsg, VERSION_DIGEST_SALT, VersionError, VersionKey, VersionToken,
        version_token,
    };
    use crate::entities::{
        Account, AuthFraud, Card, CardXref, Customer, DisclosureGroup, DisclosureGroupKey,
        FraudFlag, MatchStatus, NewPendingAuthDetail, NewTransaction, PA_ACCOUNT_STATUS_OCCURS,
        PendingAuthDetail, PendingAuthSummary, TranCatBalance, TranCatBalanceKey,
        TranCategoryMaster, TranCategoryRecord, TranTypeMaster, TranTypeRecord, UserSecurity,
    };
    use crate::numeric::{
        AccountGroupId, AccountId, Alnum, AuthCount, AuthId, CardNumber, CustSsn, CustomerId, Cvv,
        FicoScore, InterestRate, MerchantId, Money, NumericCode, TranCategoryCode, TranId,
        TranTypeCode, TransactionRowId, UserId,
    };
    use crate::ports::{
        AccountReader, AccountWriter, AuthorizationStore, Browse, CardFilter, CardReader,
        CardWriter, CustomerReader, CustomerWriter, Direction, DisclosureGroupReader, PortError,
        TranCatBalanceStore, TranTypeFileReader, TranTypeFilter, TranTypeMasterStore,
        TransactionReader, TransactionWriter, UserStore, XrefReader,
    };

    // Fixture card numbers: account 1 holds the first two, account 2 the third.
    const CARD_A: &str = "4111000000000001";
    const CARD_B: &str = "4111000000000002";
    const CARD_C: &str = "4111000000000003";

    fn account_id(value: u64) -> AccountId {
        AccountId::new(value).expect("an account number of at most 11 digits")
    }

    fn customer_id(value: u32) -> CustomerId {
        CustomerId::new(value).expect("a customer number of at most 9 digits")
    }

    fn category(value: u16) -> TranCategoryCode {
        TranCategoryCode::new(value).expect("a category code of at most 4 digits")
    }

    fn cents<const D: u8>(value: i64) -> Money<D> {
        Money::from_cents(value).expect("an amount within the field")
    }

    fn auth_ts() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2022, 7, 18)
            .and_then(|date| date.and_hms_opt(10, 15, 30))
            .expect("a valid timestamp")
    }

    fn account(id: u64) -> Account {
        Account {
            acct_id: account_id(id),
            acct_active_status: Alnum::move_from("Y"),
            acct_curr_bal: cents(19_400),
            acct_credit_limit: cents(2_020_000),
            acct_cash_credit_limit: cents(1_020_000),
            acct_open_date: Alnum::move_from("2014-11-20"),
            acct_expiraion_date: Alnum::move_from("2025-05-20"),
            acct_reissue_date: Alnum::move_from("2025-05-20"),
            acct_curr_cyc_credit: Money::ZERO,
            acct_curr_cyc_debit: Money::ZERO,
            acct_addr_zip: Alnum::move_from("A000000000"),
            acct_group_id: AccountGroupId::move_from(""),
        }
    }

    fn card(number: &str, account: u64) -> Card {
        Card {
            card_num: CardNumber::move_from(number),
            card_acct_id: account_id(account),
            card_cvv_cd: Cvv::new(123).expect("a three-digit code"),
            card_embossed_name: Alnum::move_from("ALEX SMITH"),
            card_expiraion_date: Alnum::move_from("2025-05-20"),
            card_active_status: Alnum::move_from("Y"),
        }
    }

    fn customer(id: u32) -> Customer {
        Customer {
            cust_id: customer_id(id),
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
            cust_ssn: CustSsn::new(20_973_888).expect("a nine-digit number"),
            cust_govt_issued_id: Alnum::move_from("00000000000049368437"),
            cust_dob_yyyy_mm_dd: Alnum::move_from("1961-06-08"),
            cust_eft_account_id: Alnum::move_from("0053581756"),
            cust_pri_card_holder_ind: Alnum::move_from("Y"),
            cust_fico_credit_score: FicoScore::new(274).expect("a three-digit score"),
        }
    }

    fn xref(number: &str, customer: u32, account: u64) -> CardXref {
        CardXref {
            xref_card_num: CardNumber::move_from(number),
            xref_cust_id: customer_id(customer),
            xref_acct_id: account_id(account),
        }
    }

    fn new_tran(id: &str, number: &str, proc_date: &str) -> NewTransaction {
        NewTransaction {
            tran_id: TranId::move_from(id),
            tran_type_cd: TranTypeCode::move_from("01"),
            tran_cat_cd: category(1),
            tran_source: Alnum::move_from("POS TERM"),
            tran_desc: Alnum::move_from("Purchase at Abshire-Lowe"),
            tran_amt: cents(5_047),
            tran_merchant_id: MerchantId::new(800_000_000).expect("a nine-digit merchant"),
            tran_merchant_name: Alnum::move_from("Abshire-Lowe"),
            tran_merchant_city: Alnum::move_from("North Enoshaven"),
            tran_merchant_zip: Alnum::move_from("72112"),
            tran_card_num: CardNumber::move_from(number),
            tran_orig_ts: Alnum::move_from("2022-06-10 19:27:53.000000"),
            tran_proc_ts: Alnum::move_from(&format!("{proc_date} 09:00:00.000000")),
        }
    }

    fn balance_key(account: u64, type_code: &str, category_code: u16) -> TranCatBalanceKey {
        TranCatBalanceKey {
            trancat_acct_id: account_id(account),
            trancat_type_cd: TranTypeCode::move_from(type_code),
            trancat_cd: category(category_code),
        }
    }

    fn balance(account: u64, type_code: &str, category_code: u16) -> TranCatBalance {
        TranCatBalance {
            key: balance_key(account, type_code, category_code),
            tran_cat_bal: cents(1_000),
        }
    }

    fn group_key() -> DisclosureGroupKey {
        DisclosureGroupKey {
            dis_acct_group_id: AccountGroupId::move_from("DEFAULT"),
            dis_tran_type_cd: TranTypeCode::move_from("01"),
            dis_tran_cat_cd: category(1),
        }
    }

    fn tran_type(code: &str, description: &str) -> TranTypeMaster {
        TranTypeMaster {
            tr_type: TranTypeCode::move_from(code),
            tr_description: description.to_owned(),
        }
    }

    fn user(id: &str, kind: &str) -> UserSecurity {
        UserSecurity {
            sec_usr_id: UserId::move_from(id),
            sec_usr_fname: Alnum::move_from("Ada"),
            sec_usr_lname: Alnum::move_from("Lovelace"),
            password_hash: "argon2id-fixture-hash".to_owned(),
            sec_usr_type: Alnum::move_from(kind),
        }
    }

    fn summary(account: u64) -> PendingAuthSummary {
        PendingAuthSummary {
            pa_acct_id: account_id(account),
            pa_cust_id: customer_id(1),
            pa_auth_status: Alnum::move_from("A"),
            pa_account_status: [Alnum::spaces(); PA_ACCOUNT_STATUS_OCCURS],
            pa_credit_limit: cents(2_020_000),
            pa_cash_limit: cents(1_020_000),
            pa_credit_balance: cents(19_400),
            pa_cash_balance: Money::ZERO,
            pa_approved_auth_cnt: AuthCount::ZERO,
            pa_declined_auth_cnt: AuthCount::ZERO,
            pa_approved_auth_amt: Money::ZERO,
            pa_declined_auth_amt: Money::ZERO,
        }
    }

    fn new_detail(account: u64, date_9c: i32, time_9c: i32) -> NewPendingAuthDetail {
        NewPendingAuthDetail {
            pa_acct_id: account_id(account),
            pa_auth_date_9c: date_9c,
            pa_auth_time_9c: time_9c,
            pa_auth_orig_date: Alnum::move_from("220718"),
            pa_auth_orig_time: Alnum::move_from("101530"),
            pa_card_num: CardNumber::move_from(CARD_A),
            pa_auth_type: Alnum::move_from("0100"),
            pa_card_expiry_date: Alnum::move_from("0525"),
            pa_message_type: Alnum::move_from("0110"),
            pa_message_source: Alnum::move_from("POS"),
            pa_auth_id_code: Alnum::move_from("101530"),
            pa_auth_resp_code: Alnum::move_from("00"),
            pa_auth_resp_reason: Alnum::move_from("0000"),
            pa_processing_code: NumericCode::new(0).expect("a six-digit code"),
            pa_transaction_amt: cents(5_047),
            pa_approved_amt: cents(5_047),
            pa_merchant_catagory_code: Alnum::move_from("5411"),
            pa_acqr_country_code: Alnum::move_from("840"),
            pa_pos_entry_mode: NumericCode::new(1).expect("a two-digit code"),
            pa_merchant_id: Alnum::move_from("800000000"),
            pa_merchant_name: Alnum::move_from("Abshire-Lowe"),
            pa_merchant_city: Alnum::move_from("Enoshaven"),
            pa_merchant_state: Alnum::move_from("NC"),
            pa_merchant_zip: Alnum::move_from("72112"),
            pa_transaction_id: Alnum::move_from("000000000000001"),
            pa_match_status: MatchStatus::Pending,
            pa_auth_fraud: FraudFlag::Blank,
            pa_fraud_rpt_date: Alnum::spaces(),
        }
    }

    fn fraud(number: &str) -> AuthFraud {
        AuthFraud {
            card_num: CardNumber::move_from(number),
            auth_ts: auth_ts(),
            auth_type: None,
            card_expiry_date: None,
            message_type: None,
            message_source: None,
            auth_id_code: None,
            auth_resp_code: None,
            auth_resp_reason: None,
            processing_code: None,
            transaction_amt: None,
            approved_amt: None,
            merchant_catagory_code: None,
            acqr_country_code: None,
            pos_entry_mode: None,
            merchant_id: None,
            merchant_name: None,
            merchant_city: None,
            merchant_state: None,
            merchant_zip: None,
            transaction_id: None,
            match_status: None,
            auth_fraud: Some(Alnum::move_from("F")),
            fraud_rpt_date: None,
            acct_id: Some(account_id(1)),
            cust_id: Some(customer_id(1)),
        }
    }

    /// A store seeded through every builder. Row identities: transactions
    /// `…02`, `…01`, `…03` get rows 1, 2, 3; details get 1 to 4, whose twin
    /// order under account 1 is 2, 3, 1.
    fn seeded() -> FakeStore {
        FakeStore::default()
            .with_account(account(1))
            .with_account(account(2))
            .with_card(card(CARD_B, 1))
            .with_card(card(CARD_A, 1))
            .with_card(card(CARD_C, 2))
            .with_customer(customer(1))
            .with_xref(xref(CARD_B, 1, 1))
            .with_xref(xref(CARD_A, 1, 1))
            .with_xref(xref(CARD_C, 2, 2))
            .with_transaction(new_tran("0000000000000002", CARD_C, "2022-07-02"))
            .with_transaction(new_tran("0000000000000001", CARD_B, "2022-07-01"))
            .with_transaction(new_tran("0000000000000003", CARD_A, "2022-07-03"))
            .with_tcatbal(balance(1, "01", 1))
            .with_tcatbal(balance(2, "01", 1))
            .with_disclosure_group(DisclosureGroup {
                key: group_key(),
                dis_int_rate: InterestRate::from_hundredths(1_500).expect("a rate of 15.00"),
            })
            .with_tran_type(tran_type("01", "Purchase"))
            .with_tran_type(tran_type("02", "Payment"))
            .with_tran_type(tran_type("03", "Credit"))
            .with_tran_category(TranCategoryMaster {
                trc_type_code: TranTypeCode::move_from("01"),
                trc_type_category: category(1),
                trc_cat_data: Alnum::move_from("Regular Sales Draft"),
            })
            .with_tran_type_record(TranTypeRecord {
                tran_type: TranTypeCode::move_from("01"),
                tran_type_desc: Alnum::move_from("Purchase"),
            })
            .with_tran_category_record(TranCategoryRecord {
                tran_type_cd: TranTypeCode::move_from("01"),
                tran_cat_cd: category(1),
                tran_cat_type_desc: Alnum::move_from("Regular Sales Draft"),
            })
            .with_user(user("ADMIN001", "A"))
            .with_user(user("USER0001", "U"))
            .with_user(user("USER0002", "U"))
            .with_summary(summary(1))
            .with_summary(summary(2))
            .with_detail(new_detail(1, 79_798, 900_000_000))
            .with_detail(new_detail(1, 79_797, 100))
            .with_detail(new_detail(1, 79_798, 800_000_000))
            .with_detail(new_detail(2, 79_798, 1))
            .with_fraud(fraud(CARD_A))
    }

    fn browse<K>(start_key: Option<K>, direction: Direction, limit: u16) -> Browse<K> {
        Browse {
            start_key,
            direction,
            limit,
        }
    }

    fn auth_ids(details: &[PendingAuthDetail]) -> Vec<i64> {
        details.iter().map(|detail| detail.auth_id.get()).collect()
    }

    /// The token of `parts` under the tests' [`version_key`].
    fn token_of(parts: &[&[u8]]) -> VersionToken {
        version_token(&version_key(), parts).expect("a digest of a few bytes")
    }

    #[test]
    fn version_token_is_sixteen_lower_hex_digits_and_deterministic() {
        let record: [&[u8]; 3] = [b"Y", b"0000000194.00", b"2025-05-20"];
        for parts in [&record[..], &[]] {
            let token = token_of(parts);
            assert_eq!(token, token_of(parts), "equal inputs, equal tokens");
            assert_eq!(token.as_str().len(), 16);
            assert!(
                token
                    .as_str()
                    .bytes()
                    .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')),
                "{token}"
            );
            assert_eq!(token.to_string(), token.as_str());
            assert_eq!(VersionToken::parse(token.as_str()), Some(token));
        }
        let key = version_key();
        assert_eq!(
            version_token(&key.clone(), &record),
            version_token(&key, &record),
            "a cloned key, the same token"
        );
        assert_eq!(
            version_token(&key, &record),
            Ok(token_of(&record)),
            "a key built again from the same secret, the same token"
        );
    }

    #[test]
    fn version_token_changes_with_part_boundaries_bytes_and_order() {
        let base = token_of(&[b"ab", b"c"]);
        let others: [&[&[u8]]; 5] = [
            &[b"a", b"bc"],
            &[b"abc"],
            &[b"ab", b"c", b""],
            &[b"ab", b"d"],
            &[b"c", b"ab"],
        ];
        for parts in others {
            assert_ne!(token_of(parts), base, "{parts:?}");
        }
    }

    #[test]
    fn version_token_differs_between_keys() {
        // Without the server's secret no token can be computed again.
        let other = VersionKey::new(b"another server secret").expect("a secret");
        let record: [&[u8]; 3] = [b"747", b"ANIYA VON", b"Y"];
        for parts in [&record[..], &[b"ab", b"c"], &[]] {
            assert_ne!(
                version_token(&other, parts),
                Ok(token_of(parts)),
                "{parts:?}"
            );
        }
    }

    #[test]
    fn version_key_rejects_an_empty_secret() {
        assert_eq!(VersionKey::new(b"").err(), Some(VersionError::EmptyKey));
        assert!(VersionKey::new(b"k").is_ok(), "any non-empty secret keys");
    }

    #[test]
    fn version_key_debug_never_shows_the_secret() {
        let secret = "a server secret nobody may log";
        let key = VersionKey::new(secret.as_bytes()).expect("a secret");
        for shown in [format!("{key:?}"), format!("{key:#?}")] {
            assert_eq!(shown, "VersionKey(<redacted>)");
            assert!(!shown.contains(secret), "{shown}");
        }
    }

    #[test]
    fn version_token_is_keyed_argon2id_of_the_length_prefixed_parts() {
        // The documented construction, computed with argon2 directly.
        let secret = b"an independent check secret";
        let parts: [&[u8]; 3] = [b"747", b"ANIYA", b""];
        let mut encoded = Vec::new();
        for (len, part) in [(3u64, parts[0]), (5, parts[1]), (0, parts[2])] {
            encoded.extend_from_slice(&len.to_le_bytes());
            encoded.extend_from_slice(part);
        }
        let salt = b"carddemo record version token";
        assert_eq!(VERSION_DIGEST_SALT, salt);
        assert!(salt.len() >= argon2::MIN_SALT_LEN);
        let params = Params::new(8, 1, 1, Some(8)).expect("the minimum costs");
        let argon2 = Argon2::new_with_secret(secret, Algorithm::Argon2id, Version::V0x13, params)
            .expect("a short secret");
        let mut tag = [0u8; 8];
        argon2
            .hash_password_into(&encoded, salt, &mut tag)
            .expect("a short input");
        let expected = format!("{:016x}", u64::from_be_bytes(tag));

        let key = VersionKey::new(secret).expect("a secret");
        let token = version_token(&key, &parts).expect("a digest of a few bytes");
        assert_eq!(token.as_str(), expected);
    }

    #[test]
    fn version_token_parse_accepts_only_sixteen_lower_hex_digits() {
        let cases = [
            ("0123456789abcdef", true),
            ("ffffffffffffffff", true),
            ("0123456789ABCDEF", false),
            ("0123456789abcde", false),
            ("0123456789abcdef0", false),
            ("0123456789abcdeg", false),
            (" 123456789abcdef", false),
            ("", false),
        ];
        for (text, accepted) in cases {
            let parsed = VersionToken::parse(text);
            assert_eq!(parsed.is_some(), accepted, "{text:?}");
            if let Some(token) = parsed {
                assert_eq!(token.as_str(), text);
            }
        }
    }

    #[test]
    fn confirm_parse_classifies_every_input_class() {
        let cases = [
            (None, Confirm::Blank),
            (Some(""), Confirm::Blank),
            (Some(" "), Confirm::Blank),
            (Some("   "), Confirm::Blank),
            (Some("\0"), Confirm::Blank),
            (Some("Y"), Confirm::Yes),
            (Some("y"), Confirm::Yes),
            (Some("N"), Confirm::No),
            (Some("n"), Confirm::No),
            (Some("X"), Confirm::Other("X".to_owned())),
            (Some("YY"), Confirm::Other("YY".to_owned())),
            (Some("Y "), Confirm::Other("Y ".to_owned())),
            (Some(" \0"), Confirm::Other(" \0".to_owned())),
        ];
        for (input, expected) in cases {
            assert_eq!(Confirm::parse(input), expected, "{input:?}");
        }
    }

    #[test]
    fn return_msg_keeps_the_first_message_and_its_field() {
        let mut msg = ReturnMsg::default();
        assert!(msg.is_off());
        assert_eq!((msg.message(), msg.field()), (None, None));

        msg.set_if_off("Credit Limit is not valid", "ACRDLIM");
        msg.set_if_off(
            String::from("Account Active Status must be Y or N"),
            "ACSTTUS",
        );

        assert!(!msg.is_off());
        assert_eq!(msg.message(), Some("Credit Limit is not valid"));
        assert_eq!(msg.field(), Some("ACRDLIM"));
        assert_eq!(
            msg.into_parts(),
            Some(("Credit Limit is not valid".to_owned(), "ACRDLIM"))
        );
    }

    #[test]
    fn return_msg_stays_off_for_a_blank_message() {
        let mut msg = ReturnMsg::default();
        msg.set_if_off("", "ACSTTUS");
        msg.set_if_off("   ", "ACSTTUS");
        assert!(msg.is_off());
        assert_eq!(msg.clone().into_parts(), None);

        msg.set_if_off("Account Active Status must be Y or N", "ACSTTUS");
        assert_eq!(msg.field(), Some("ACSTTUS"));
    }

    /// Pending until another thread sets its flag and wakes it.
    struct WokenFromThread {
        done: Arc<AtomicBool>,
        started: bool,
    }

    impl Future for WokenFromThread {
        type Output = &'static str;

        fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
            if self.done.load(Ordering::SeqCst) {
                return Poll::Ready("woken");
            }
            if !self.started {
                self.started = true;
                let done = Arc::clone(&self.done);
                let waker = context.waker().clone();
                thread::spawn(move || {
                    thread::sleep(Duration::from_millis(20));
                    done.store(true, Ordering::SeqCst);
                    waker.wake();
                });
            }
            Poll::Pending
        }
    }

    #[test]
    fn block_on_drives_ready_and_pending_futures() {
        assert_eq!(block_on(ready(5)), 5);
        let woken = WokenFromThread {
            done: Arc::new(AtomicBool::new(false)),
            started: false,
        };
        assert_eq!(block_on(woken), "woken");
        // Every FakeStore call is pending once before it answers.
        let mut store = seeded();
        assert_eq!(block_on(store.read_account(account_id(1))), Ok(account(1)));
    }

    /// The display text of each row's key, for order assertions.
    fn texts<T, D: std::fmt::Display>(rows: &[T], key: impl Fn(&T) -> D) -> Vec<String> {
        rows.iter().map(|row| key(row).to_string()).collect()
    }

    /// Queues a failure named for `$method`, makes the call `$call` and
    /// asserts that it returns exactly that failure.
    macro_rules! fails_as_injected {
        ($store:ident, $method:literal, $call:expr) => {{
            let error = PortError::Unavailable($method.to_owned());
            $store.fail_next($method, error.clone());
            assert_eq!(block_on($call).err(), Some(error), $method);
        }};
    }

    fn inject_into_files(store: &mut FakeStore) {
        let card_a = CardNumber::move_from(CARD_A);
        let first = TranId::move_from("0000000000000001");
        let all_cards = browse(None, Direction::Forward, 10);
        let all_trans = browse(None, Direction::Forward, 10);
        let (from, to) = (
            Alnum::move_from("2022-07-01"),
            Alnum::move_from("2022-07-03"),
        );
        let added = new_tran("0000000000000004", CARD_A, "2022-07-04");
        fails_as_injected!(store, "read_account", store.read_account(account_id(1)));
        fails_as_injected!(
            store,
            "read_account_for_update",
            store.read_account_for_update(account_id(1))
        );
        fails_as_injected!(store, "rewrite_account", store.rewrite_account(account(1)));
        fails_as_injected!(store, "read_card", store.read_card(card_a));
        fails_as_injected!(
            store,
            "read_card_for_update",
            store.read_card_for_update(card_a)
        );
        fails_as_injected!(
            store,
            "browse_cards",
            store.browse_cards(all_cards, CardFilter::default())
        );
        fails_as_injected!(
            store,
            "cards_for_account",
            store.cards_for_account(account_id(1))
        );
        fails_as_injected!(store, "rewrite_card", store.rewrite_card(card(CARD_A, 1)));
        fails_as_injected!(store, "read_customer", store.read_customer(customer_id(1)));
        fails_as_injected!(
            store,
            "read_customer_for_update",
            store.read_customer_for_update(customer_id(1))
        );
        fails_as_injected!(
            store,
            "rewrite_customer",
            store.rewrite_customer(customer(1))
        );
        fails_as_injected!(store, "read_xref_by_card", store.read_xref_by_card(card_a));
        fails_as_injected!(
            store,
            "read_xref_by_account",
            store.read_xref_by_account(account_id(1))
        );
        fails_as_injected!(store, "xrefs_after", store.xrefs_after(None, 10));
        fails_as_injected!(store, "read_transaction", store.read_transaction(first));
        fails_as_injected!(
            store,
            "browse_transactions",
            store.browse_transactions(all_trans, None)
        );
        fails_as_injected!(store, "highest_transaction", store.highest_transaction());
        fails_as_injected!(
            store,
            "transactions_by_card",
            store.transactions_by_card(10)
        );
        fails_as_injected!(
            store,
            "transactions_by_proc_date",
            store.transactions_by_proc_date(from, to, None, 10)
        );
        fails_as_injected!(store, "write_transaction", store.write_transaction(added));
        fails_as_injected!(
            store,
            "read_tcatbal",
            store.read_tcatbal(balance_key(1, "01", 1))
        );
        fails_as_injected!(
            store,
            "write_tcatbal",
            store.write_tcatbal(balance(3, "01", 1))
        );
        fails_as_injected!(
            store,
            "rewrite_tcatbal",
            store.rewrite_tcatbal(balance(1, "01", 1))
        );
        fails_as_injected!(store, "tcatbals_after", store.tcatbals_after(None, 10));
        fails_as_injected!(
            store,
            "read_disclosure_group",
            store.read_disclosure_group(group_key())
        );
    }

    fn inject_into_reference_data_and_users(store: &mut FakeStore) {
        let purchase = TranTypeCode::move_from("01");
        let all_types = browse(None, Direction::Forward, 10);
        let admin = UserId::move_from("ADMIN001");
        let all_users = browse(None, Direction::Forward, 10);
        fails_as_injected!(store, "read_tran_type", store.read_tran_type(purchase));
        fails_as_injected!(
            store,
            "browse_tran_types",
            store.browse_tran_types(all_types, TranTypeFilter::default())
        );
        fails_as_injected!(
            store,
            "count_tran_types",
            store.count_tran_types(TranTypeFilter::default())
        );
        fails_as_injected!(
            store,
            "insert_tran_type",
            store.insert_tran_type(tran_type("04", "Authorization"))
        );
        fails_as_injected!(
            store,
            "update_tran_type",
            store.update_tran_type(tran_type("01", "PURCHASE"))
        );
        fails_as_injected!(
            store,
            "delete_tran_type",
            store.delete_tran_type(TranTypeCode::move_from("02"))
        );
        fails_as_injected!(
            store,
            "read_tran_type_file",
            store.read_tran_type_file(purchase)
        );
        fails_as_injected!(
            store,
            "read_tran_category_file",
            store.read_tran_category_file(purchase, category(1))
        );
        fails_as_injected!(store, "read_user", store.read_user(admin));
        fails_as_injected!(
            store,
            "read_user_for_update",
            store.read_user_for_update(admin)
        );
        fails_as_injected!(store, "browse_users", store.browse_users(all_users));
        fails_as_injected!(
            store,
            "insert_user",
            store.insert_user(user("USER0003", "U"))
        );
        fails_as_injected!(
            store,
            "update_user",
            store.update_user(user("USER0001", "A"))
        );
        fails_as_injected!(
            store,
            "delete_user",
            store.delete_user(UserId::move_from("USER0002"))
        );
    }

    fn inject_into_authorizations(store: &mut FakeStore) {
        let first = AuthId::from(1);
        let stored = PendingAuthDetail::from_new(first, new_detail(1, 79_798, 900_000_000));
        let all_twins = browse(None, Direction::Forward, 10);
        fails_as_injected!(store, "read_summary", store.read_summary(account_id(1)));
        fails_as_injected!(
            store,
            "read_summary_of_detail",
            store.read_summary_of_detail(first)
        );
        fails_as_injected!(store, "insert_summary", store.insert_summary(summary(3)));
        fails_as_injected!(store, "update_summary", store.update_summary(summary(1)));
        fails_as_injected!(store, "delete_summary", store.delete_summary(account_id(2)));
        fails_as_injected!(store, "summaries_after", store.summaries_after(None, 10));
        fails_as_injected!(
            store,
            "insert_detail",
            store.insert_detail(new_detail(1, 79_796, 5))
        );
        fails_as_injected!(store, "read_detail", store.read_detail(first));
        fails_as_injected!(
            store,
            "read_detail_for_update",
            store.read_detail_for_update(first)
        );
        fails_as_injected!(
            store,
            "details_for_account",
            store.details_for_account(account_id(1), all_twins)
        );
        fails_as_injected!(
            store,
            "adjacent_detail",
            store.adjacent_detail(account_id(1), 79_798, 900_000_000, Direction::Forward)
        );
        fails_as_injected!(store, "update_detail", store.update_detail(stored));
        fails_as_injected!(store, "delete_detail", store.delete_detail(first));
        fails_as_injected!(store, "insert_fraud", store.insert_fraud(fraud(CARD_B)));
        fails_as_injected!(store, "update_fraud", store.update_fraud(fraud(CARD_A)));
    }

    #[test]
    fn every_port_method_consumes_its_own_injected_failure_and_changes_nothing() {
        let mut store = seeded();
        inject_into_files(&mut store);
        inject_into_reference_data_and_users(&mut store);
        inject_into_authorizations(&mut store);

        assert_eq!(store.calls, PORT_METHODS, "each call logs its own name");
        store.calls.clear();
        assert_eq!(
            store,
            seeded(),
            "no failure is left over and nothing changed"
        );
    }

    #[test]
    fn injected_failures_fire_in_order_for_their_method_only() {
        let mut store = seeded();
        store.fail_next("read_account", PortError::Contention);
        store.fail_next("read_account", PortError::NotFound);

        let card_a = CardNumber::move_from(CARD_A);
        assert_eq!(block_on(store.read_card(card_a)), Ok(card(CARD_A, 1)));
        let id = account_id(1);
        assert_eq!(block_on(store.read_account(id)), Err(PortError::Contention));
        assert_eq!(block_on(store.read_account(id)), Err(PortError::NotFound));
        assert_eq!(block_on(store.read_account(id)), Ok(account(1)));
        assert_eq!(
            store.calls,
            ["read_card", "read_account", "read_account", "read_account"]
        );
    }

    #[test]
    #[should_panic(expected = "no port method is named \"read_acount\"")]
    fn fail_next_rejects_a_name_no_port_method_has() {
        FakeStore::default().fail_next("read_acount", PortError::NotFound);
    }

    #[test]
    fn master_files_read_and_rewrite_by_key() {
        let mut store = seeded();
        let missing = account_id(9);
        assert_eq!(
            block_on(store.read_account(missing)),
            Err(PortError::NotFound)
        );
        assert_eq!(
            block_on(store.rewrite_account(account(9))),
            Err(PortError::NotFound)
        );
        let mut changed = account(1);
        changed.acct_curr_bal = Money::ZERO;
        assert_eq!(block_on(store.rewrite_account(changed.clone())), Ok(()));
        assert_eq!(
            block_on(store.read_account_for_update(account_id(1))),
            Ok(changed)
        );

        let missing_card = card("4111000000000009", 1);
        assert_eq!(
            block_on(store.rewrite_card(missing_card)),
            Err(PortError::NotFound)
        );
        let mut renamed = card(CARD_B, 1);
        renamed.card_embossed_name = Alnum::move_from("ALEX J SMITH");
        assert_eq!(block_on(store.rewrite_card(renamed.clone())), Ok(()));
        let card_b = CardNumber::move_from(CARD_B);
        assert_eq!(block_on(store.read_card_for_update(card_b)), Ok(renamed));

        assert_eq!(
            block_on(store.read_customer(customer_id(9))),
            Err(PortError::NotFound)
        );
        assert_eq!(
            block_on(store.rewrite_customer(customer(9))),
            Err(PortError::NotFound)
        );
        let mut moved = customer(1);
        moved.cust_addr_zip = Alnum::move_from("27601");
        assert_eq!(block_on(store.rewrite_customer(moved.clone())), Ok(()));
        assert_eq!(
            block_on(store.read_customer_for_update(customer_id(1))),
            Ok(moved)
        );
    }

    #[test]
    fn card_browse_follows_the_browse_contract_and_skips_filtered_cards() {
        let mut store = seeded();
        let card_b = CardNumber::move_from(CARD_B);
        let cases = [
            (
                browse(None, Direction::Forward, 2),
                CardFilter::default(),
                vec![CARD_A, CARD_B],
            ),
            (
                browse(Some(card_b), Direction::Forward, 10),
                CardFilter::default(),
                vec![CARD_B, CARD_C],
            ),
            (
                browse(Some(card_b), Direction::Backward, 10),
                CardFilter::default(),
                vec![CARD_B, CARD_A],
            ),
            (
                browse(None, Direction::Backward, 1),
                CardFilter::default(),
                vec![CARD_C],
            ),
            (
                browse(None, Direction::Forward, 1),
                CardFilter {
                    account_id: Some(account_id(2)),
                    card_number: None,
                },
                vec![CARD_C],
            ),
            (
                browse(None, Direction::Forward, 10),
                CardFilter {
                    account_id: Some(account_id(1)),
                    card_number: Some(card_b),
                },
                vec![CARD_B],
            ),
        ];
        for (page, filter, expected) in cases {
            let rows = block_on(store.browse_cards(page.clone(), filter.clone()))
                .expect("a browse of the fake store");
            assert_eq!(
                texts(&rows, |row| row.card_num),
                expected,
                "{page:?} {filter:?}"
            );
        }
        let own = block_on(store.cards_for_account(account_id(1))).expect("a path read");
        assert_eq!(texts(&own, |row| row.card_num), [CARD_A, CARD_B]);
        let none = block_on(store.cards_for_account(account_id(9))).expect("a path read");
        assert!(none.is_empty());
    }

    #[test]
    fn cross_references_answer_both_paths_and_the_sweep() {
        let mut store = seeded();
        let card_b = CardNumber::move_from(CARD_B);
        assert_eq!(
            block_on(store.read_xref_by_card(card_b)),
            Ok(xref(CARD_B, 1, 1))
        );
        let unknown = CardNumber::move_from("4111000000000009");
        assert_eq!(
            block_on(store.read_xref_by_card(unknown)),
            Err(PortError::NotFound)
        );
        assert_eq!(
            block_on(store.read_xref_by_account(account_id(1))),
            Ok(xref(CARD_A, 1, 1)),
            "the first cross-reference of the account in card-number order"
        );
        assert_eq!(
            block_on(store.read_xref_by_account(account_id(9))),
            Err(PortError::NotFound)
        );

        let sweeps = [
            (None, 2, vec![CARD_A, CARD_B]),
            (
                Some(CardNumber::move_from(CARD_A)),
                10,
                vec![CARD_B, CARD_C],
            ),
            (Some(CardNumber::move_from(CARD_C)), 10, vec![]),
        ];
        for (after, limit, expected) in sweeps {
            let rows = block_on(store.xrefs_after(after, limit)).expect("a sweep");
            assert_eq!(texts(&rows, |row| row.xref_card_num), expected, "{after:?}");
        }
    }

    #[test]
    fn transaction_master_writes_take_the_next_row_identity() {
        let mut store = seeded();
        let first = TranId::move_from("0000000000000001");
        let stored = block_on(store.read_transaction(first)).expect("a seeded transaction");
        assert_eq!(stored.transaction_id, TransactionRowId::from(2));
        assert_eq!(
            stored.to_new(),
            new_tran("0000000000000001", CARD_B, "2022-07-01")
        );

        let again = new_tran("0000000000000001", CARD_A, "2022-07-09");
        assert_eq!(
            block_on(store.write_transaction(again)),
            Err(PortError::Duplicate)
        );
        let added = new_tran("0000000000000004", CARD_A, "2022-07-04");
        assert_eq!(
            block_on(store.write_transaction(added)),
            Ok(TransactionRowId::from(4))
        );
        let highest = block_on(store.highest_transaction()).expect("a highest-key read");
        assert_eq!(
            highest.map(|tran| tran.tran_id),
            Some(TranId::move_from("0000000000000004"))
        );
        let missing = TranId::move_from("0000000000000009");
        assert_eq!(
            block_on(store.read_transaction(missing)),
            Err(PortError::NotFound)
        );
        let mut empty = FakeStore::default();
        assert_eq!(block_on(empty.highest_transaction()), Ok(None));
    }

    #[test]
    fn transaction_browse_and_selections_keep_their_orders() {
        let mut store = seeded();
        let second = TranId::move_from("0000000000000002");
        let cases = [
            (browse(None, Direction::Forward, 2), None, vec!["1", "2"]),
            (
                browse(Some(second), Direction::Backward, 10),
                None,
                vec!["2", "1"],
            ),
            (
                browse(None, Direction::Forward, 10),
                Some(account_id(1)),
                vec!["1", "3"],
            ),
            (
                browse(None, Direction::Forward, 1),
                Some(account_id(2)),
                vec!["2"],
            ),
            (
                browse(None, Direction::Forward, 10),
                Some(account_id(9)),
                vec![],
            ),
        ];
        for (page, account, expected) in cases {
            let rows = block_on(store.browse_transactions(page.clone(), account))
                .expect("a browse of the fake store");
            let ids = texts(&rows, |row| {
                row.tran_id.trimmed().trim_start_matches('0').to_owned()
            });
            assert_eq!(ids, expected, "{page:?} {account:?}");
        }

        let sorted = [
            format!("{CARD_A}/0000000000000003"),
            format!("{CARD_B}/0000000000000001"),
            format!("{CARD_C}/0000000000000002"),
        ];
        // A limit past the rows reads them all; one below stops after the
        // first rows of the card-number order.
        for (limit, expected) in [(10, &sorted[..]), (3, &sorted[..]), (2, &sorted[..2])] {
            let by_card = block_on(store.transactions_by_card(limit)).expect("a sorted read");
            let order = texts(&by_card, |row| {
                format!("{}/{}", row.tran_card_num, row.tran_id)
            });
            assert_eq!(order, expected, "limit {limit}");
        }
    }

    #[test]
    fn the_date_selection_pages_in_card_then_tran_id_order() {
        // Rows after the `after` pair only, at most `limit` of them.
        let mut store = seeded();
        let key = |card: &str, id: &str| Some((CardNumber::move_from(card), TranId::move_from(id)));
        let (a3, b1, c2) = (
            format!("{CARD_A}/0000000000000003"),
            format!("{CARD_B}/0000000000000001"),
            format!("{CARD_C}/0000000000000002"),
        );
        let (a3, b1, c2) = (a3.as_str(), b1.as_str(), c2.as_str());
        let ranges = [
            ("2022-07-02", "2022-07-03", None, 10, vec![a3, c2]),
            ("2022-07-01", "2022-07-01", None, 10, vec![b1]),
            ("2022-07-05", "2022-07-09", None, 10, vec![]),
            ("2022-07-01", "2022-07-03", None, 2, vec![a3, b1]),
            ("2022-07-01", "2022-07-03", None, 0, vec![]),
            (
                "2022-07-01",
                "2022-07-03",
                key(CARD_A, "0000000000000003"),
                1,
                vec![b1],
            ),
            (
                "2022-07-01",
                "2022-07-03",
                key(CARD_B, "0000000000000001"),
                10,
                vec![c2],
            ),
            (
                "2022-07-01",
                "2022-07-03",
                key(CARD_B, "0000000000000000"),
                10,
                vec![b1, c2],
            ),
            (
                "2022-07-01",
                "2022-07-03",
                key(CARD_C, "0000000000000002"),
                10,
                vec![],
            ),
        ];
        for (start, end, after, limit, expected) in ranges {
            let rows = block_on(store.transactions_by_proc_date(
                Alnum::move_from(start),
                Alnum::move_from(end),
                after,
                limit,
            ))
            .expect("a date selection");
            let order = texts(&rows, |row| {
                format!("{}/{}", row.tran_card_num, row.tran_id)
            });
            assert_eq!(
                order, expected,
                "{start}..={end} after {after:?} limit {limit}"
            );
        }
    }

    #[test]
    fn category_balances_and_disclosure_groups_by_key() {
        let mut store = seeded();
        assert_eq!(
            block_on(store.read_tcatbal(balance_key(1, "01", 1))),
            Ok(balance(1, "01", 1))
        );
        assert_eq!(
            block_on(store.read_tcatbal(balance_key(1, "02", 1))),
            Err(PortError::NotFound)
        );
        assert_eq!(
            block_on(store.write_tcatbal(balance(1, "01", 1))),
            Err(PortError::Duplicate)
        );
        assert_eq!(block_on(store.write_tcatbal(balance(1, "02", 3))), Ok(()));
        assert_eq!(
            block_on(store.rewrite_tcatbal(balance(9, "01", 1))),
            Err(PortError::NotFound)
        );
        let mut grown = balance(2, "01", 1);
        grown.tran_cat_bal = cents(2_500);
        assert_eq!(block_on(store.rewrite_tcatbal(grown.clone())), Ok(()));

        let swept =
            block_on(store.tcatbals_after(Some(balance_key(1, "01", 1)), 10)).expect("a sweep");
        assert_eq!(swept, [balance(1, "02", 3), grown]);
        let head = block_on(store.tcatbals_after(None, 1)).expect("a sweep");
        assert_eq!(head, [balance(1, "01", 1)]);

        let group = block_on(store.read_disclosure_group(group_key())).expect("a seeded group");
        assert_eq!(group.dis_int_rate.hundredths(), 1_500);
        let mut other = group_key();
        other.dis_acct_group_id = AccountGroupId::move_from("GOLD");
        assert_eq!(
            block_on(store.read_disclosure_group(other)),
            Err(PortError::NotFound)
        );
    }

    #[test]
    fn type_master_cursors_bound_backward_reads_strictly() {
        let mut store = seeded();
        let code = TranTypeCode::move_from;
        let cases = [
            (
                browse(Some(code("02")), Direction::Forward, 10),
                vec!["02", "03"],
            ),
            (
                browse(Some(code("02")), Direction::Backward, 10),
                vec!["01"],
            ),
            (browse(Some(code("  ")), Direction::Backward, 10), vec![]),
            (browse(None, Direction::Backward, 2), vec!["03", "02"]),
        ];
        for (page, expected) in cases {
            let rows = block_on(store.browse_tran_types(page.clone(), TranTypeFilter::default()))
                .expect("a cursor of the fake store");
            assert_eq!(texts(&rows, |row| row.tr_type), expected, "{page:?}");
        }

        let filters = [
            (Some(code("02")), None, vec!["02"]),
            (None, Some("%r%"), vec!["01", "03"]),
            (None, Some("P_y%"), vec!["02"]),
            (None, Some("%R%"), vec![]),
            (None, Some("Credit"), vec!["03"]),
            (None, Some("Credi"), vec![]),
        ];
        for (type_code, description, expected) in filters {
            let filter = TranTypeFilter {
                type_code,
                description: description.map(str::to_owned),
            };
            let all = browse(None, Direction::Forward, 10);
            let rows = block_on(store.browse_tran_types(all, filter.clone()))
                .expect("a cursor of the fake store");
            assert_eq!(texts(&rows, |row| row.tr_type), expected, "{filter:?}");
            let count = block_on(store.count_tran_types(filter)).expect("a count");
            assert_eq!(count, u64::try_from(expected.len()).expect("a small count"));
        }
    }

    #[test]
    fn type_master_writes_and_processing_copy_reads() {
        let mut store = seeded();
        let code = TranTypeCode::move_from;
        assert_eq!(
            block_on(store.insert_tran_type(tran_type("01", "Again"))),
            Err(PortError::Duplicate)
        );
        assert_eq!(
            block_on(store.insert_tran_type(tran_type("04", "Authorization"))),
            Ok(())
        );
        assert_eq!(
            block_on(store.update_tran_type(tran_type("09", "Nothing"))),
            Err(PortError::NotFound)
        );
        assert_eq!(
            block_on(store.update_tran_type(tran_type("02", "PAYMENT"))),
            Ok(())
        );
        assert_eq!(
            block_on(store.read_tran_type(code("02"))),
            Ok(tran_type("02", "PAYMENT"))
        );
        assert_eq!(
            block_on(store.delete_tran_type(code("01"))),
            Err(PortError::Restricted),
            "a category still references type 01"
        );
        assert_eq!(
            block_on(store.delete_tran_type(code("09"))),
            Err(PortError::NotFound)
        );
        assert_eq!(block_on(store.delete_tran_type(code("03"))), Ok(()));
        assert_eq!(
            block_on(store.read_tran_type(code("03"))),
            Err(PortError::NotFound)
        );

        let record = block_on(store.read_tran_type_file(code("01"))).expect("a seeded record");
        assert_eq!(record.tran_type_desc, Alnum::move_from("Purchase"));
        assert_eq!(
            block_on(store.read_tran_type_file(code("02"))),
            Err(PortError::NotFound),
            "the processing copy is separate from the master"
        );
        let found = block_on(store.read_tran_category_file(code("01"), category(1)))
            .expect("a seeded record");
        assert_eq!(
            found.tran_cat_type_desc,
            Alnum::move_from("Regular Sales Draft")
        );
        assert_eq!(
            block_on(store.read_tran_category_file(code("01"), category(2))),
            Err(PortError::NotFound)
        );
    }

    #[test]
    fn user_file_browses_and_writes_by_user_id() {
        let mut store = seeded();
        let id = UserId::move_from;
        let cases = [
            (
                browse(Some(id("USER0001")), Direction::Forward, 10),
                vec!["USER0001", "USER0002"],
            ),
            (
                browse(Some(id("USER0001")), Direction::Backward, 10),
                vec!["USER0001", "ADMIN001"],
            ),
            (
                browse(None, Direction::Backward, 2),
                vec!["USER0002", "USER0001"],
            ),
            (browse(Some(id("USER9999")), Direction::Forward, 10), vec![]),
        ];
        for (page, expected) in cases {
            let rows = block_on(store.browse_users(page.clone())).expect("a browse");
            assert_eq!(texts(&rows, |row| row.sec_usr_id), expected, "{page:?}");
        }

        assert_eq!(
            block_on(store.read_user(id("NOBODY"))),
            Err(PortError::NotFound)
        );
        assert_eq!(
            block_on(store.insert_user(user("USER0001", "U"))),
            Err(PortError::Duplicate)
        );
        assert_eq!(block_on(store.insert_user(user("USER0003", "U"))), Ok(()));
        assert_eq!(
            block_on(store.update_user(user("NOBODY", "U"))),
            Err(PortError::NotFound)
        );
        assert_eq!(block_on(store.update_user(user("USER0003", "A"))), Ok(()));
        let promoted = block_on(store.read_user_for_update(id("USER0003"))).expect("a user");
        assert!(promoted.is_admin());
        assert_eq!(
            block_on(store.delete_user(id("NOBODY"))),
            Err(PortError::NotFound)
        );
        assert_eq!(block_on(store.delete_user(id("USER0003"))), Ok(()));
        assert!(!store.users.contains_key(&id("USER0003")));
    }

    #[test]
    fn summaries_sweep_in_account_order_and_delete_with_their_details() {
        let mut store = seeded();
        assert_eq!(block_on(store.read_summary(account_id(1))), Ok(summary(1)));
        assert_eq!(
            block_on(store.read_summary(account_id(9))),
            Err(PortError::NotFound)
        );
        assert_eq!(
            block_on(store.insert_summary(summary(1))),
            Err(PortError::Duplicate)
        );
        assert_eq!(block_on(store.insert_summary(summary(3))), Ok(()));
        assert_eq!(
            block_on(store.update_summary(summary(9))),
            Err(PortError::NotFound)
        );
        let mut counted = summary(2);
        counted.pa_approved_auth_cnt = AuthCount::new(1).expect("a count of one");
        assert_eq!(block_on(store.update_summary(counted.clone())), Ok(()));

        let swept = block_on(store.summaries_after(Some(account_id(1)), 10)).expect("a sweep");
        assert_eq!(swept, [counted, summary(3)]);

        assert_eq!(block_on(store.delete_summary(account_id(1))), Ok(()));
        let left: Vec<i64> = store.details.keys().map(|auth_id| auth_id.get()).collect();
        assert_eq!(
            left,
            [4],
            "the details of account 1 went with their summary"
        );
        assert_eq!(
            block_on(store.delete_summary(account_id(1))),
            Err(PortError::NotFound)
        );
    }

    #[test]
    fn details_read_in_twin_order_from_an_inclusive_start() {
        let mut store = seeded();
        let id = AuthId::from;
        let cases = [
            (browse(None, Direction::Forward, 10), vec![2, 3, 1]),
            (browse(None, Direction::Forward, 1), vec![2]),
            (browse(Some(id(3)), Direction::Forward, 10), vec![3, 1]),
            (browse(None, Direction::Backward, 10), vec![1, 3, 2]),
            (browse(Some(id(3)), Direction::Backward, 10), vec![3, 2]),
            (browse(Some(id(4)), Direction::Forward, 10), vec![]),
        ];
        for (page, expected) in cases {
            let rows = block_on(store.details_for_account(account_id(1), page.clone()))
                .expect("a twin read");
            assert_eq!(auth_ids(&rows), expected, "{page:?}");
        }

        // From the keys of details 2, 1, 2, 3 and 4, a key between details 3
        // and 1 that no detail holds, and an account with no twins.
        let steps = [
            (1, 79_797, 100, Direction::Forward, Some(3)),
            (1, 79_798, 900_000_000, Direction::Forward, None),
            (1, 79_797, 100, Direction::Backward, None),
            (1, 79_798, 800_000_000, Direction::Backward, Some(2)),
            (2, 79_798, 1, Direction::Forward, None),
            (1, 79_798, 850_000_000, Direction::Forward, Some(1)),
            (1, 79_798, 850_000_000, Direction::Backward, Some(3)),
            (9, 0, 0, Direction::Forward, None),
        ];
        for (account, date_9c, time_9c, direction, expected) in steps {
            let next =
                block_on(store.adjacent_detail(account_id(account), date_9c, time_9c, direction))
                    .expect("a twin read");
            assert_eq!(
                next.map(|detail| detail.auth_id.get()),
                expected,
                "{account} {date_9c} {time_9c} {direction:?}"
            );
        }
    }

    #[test]
    fn the_summary_of_a_detail_is_its_parent_and_the_locking_read_is_the_detail() {
        let mut store = seeded().with_detail(new_detail(9, 79_798, 1));
        assert_eq!(
            block_on(store.read_summary_of_detail(AuthId::from(4))),
            Ok(summary(2))
        );
        assert_eq!(
            block_on(store.read_summary_of_detail(AuthId::from(99))),
            Err(PortError::NotFound),
            "no detail has the identity"
        );
        assert_eq!(
            block_on(store.read_summary_of_detail(AuthId::from(5))),
            Err(PortError::NotFound),
            "the detail's account has no summary"
        );
        let first = block_on(store.read_detail(AuthId::from(1))).expect("a seeded detail");
        assert_eq!(
            block_on(store.read_detail_for_update(AuthId::from(1))),
            Ok(first)
        );
        assert_eq!(
            block_on(store.read_detail_for_update(AuthId::from(99))),
            Err(PortError::NotFound)
        );
    }

    #[test]
    fn detail_writes_follow_the_ims_parent_and_key_rules() {
        let mut store = seeded();
        assert_eq!(
            block_on(store.insert_detail(new_detail(9, 79_796, 5))),
            Err(PortError::NotFound),
            "no summary to insert under"
        );
        assert_eq!(
            block_on(store.insert_detail(new_detail(1, 79_797, 100))),
            Err(PortError::Duplicate),
            "the same PAUT9CTS key under the same summary"
        );
        assert_eq!(
            block_on(store.insert_detail(new_detail(2, 79_797, 100))),
            Ok(AuthId::from(5)),
            "the same key under another summary"
        );
        let all = browse(None, Direction::Forward, 10);
        let twins = block_on(store.details_for_account(account_id(2), all)).expect("a twin read");
        assert_eq!(auth_ids(&twins), [5, 4]);

        let mut marked = block_on(store.read_detail(AuthId::from(1))).expect("a seeded detail");
        marked.pa_auth_fraud = FraudFlag::Confirmed;
        assert_eq!(block_on(store.update_detail(marked.clone())), Ok(()));
        assert_eq!(block_on(store.read_detail(AuthId::from(1))), Ok(marked));
        let unknown = PendingAuthDetail::from_new(AuthId::from(99), new_detail(1, 1, 1));
        assert_eq!(
            block_on(store.update_detail(unknown)),
            Err(PortError::NotFound)
        );
        assert_eq!(block_on(store.delete_detail(AuthId::from(1))), Ok(()));
        assert_eq!(
            block_on(store.delete_detail(AuthId::from(1))),
            Err(PortError::NotFound)
        );
        assert_eq!(
            block_on(store.read_detail(AuthId::from(1))),
            Err(PortError::NotFound)
        );
    }

    #[test]
    fn fraud_reports_insert_once_and_update_only_their_flag_and_date() {
        let mut store = seeded();
        assert_eq!(
            block_on(store.insert_fraud(fraud(CARD_A))),
            Err(PortError::Duplicate)
        );
        assert_eq!(block_on(store.insert_fraud(fraud(CARD_B))), Ok(()));

        let report_date = NaiveDate::from_ymd_opt(2022, 7, 19).expect("a calendar date");
        let mut removal = fraud(CARD_A);
        removal.auth_fraud = Some(Alnum::move_from("R"));
        removal.fraud_rpt_date = Some(report_date);
        removal.acct_id = None;
        assert_eq!(block_on(store.update_fraud(removal)), Ok(()));
        let stored = &store.frauds[&(CardNumber::move_from(CARD_A), auth_ts())];
        assert_eq!(stored.auth_fraud, Some(Alnum::move_from("R")));
        assert_eq!(stored.fraud_rpt_date, Some(report_date));
        assert_eq!(
            stored.acct_id,
            Some(account_id(1)),
            "other columns are kept"
        );

        assert_eq!(
            block_on(store.update_fraud(fraud(CARD_C))),
            Err(PortError::NotFound)
        );
    }
}
