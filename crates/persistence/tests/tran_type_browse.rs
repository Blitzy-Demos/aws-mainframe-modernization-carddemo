//! The `COTRTLIC` browse of the Db2 master `CARDDEMO.TRANSACTION_TYPE` in the
//! key order of its `TR_TYPE CHAR(2)` column, against a migrated, seeded
//! `PostgreSQL` 16.15.
//!
//! Units under test:
//!
//! - `PgStore::browse_tran_types`, the cursors `C-TR-TYPE-FORWARD`
//!   (`TR_TYPE >= :WS-START-KEY … ORDER BY TR_TYPE`) and `C-TR-TYPE-BACKWARD`
//!   (`TR_TYPE < :WS-START-KEY … ORDER BY TR_TYPE DESC`)
//!   [app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L338-367] — P-COTRTLIC-003,
//!   P-COTRTLIC-020, P-COTRTLIC-030, P-COTRTLIC-033;
//! - `carddemo_domain::rules::tran_type_list::list` over that store, the
//!   first page, F8 (`8000-READ-FORWARD`) and F7 (`8100-READ-BACKWARDS`) —
//!   P-COTRTLIC-030, P-COTRTLIC-031, P-COTRTLIC-033, P-COTRTLIC-034.
//!
//! The column stores the code right-trimmed, but Db2 compares `CHAR(2)` values
//! as their two blank-padded bytes, as the `Ord` of `TranTypeCode` does. Two
//! codes the seven seeded types (`01` to `07`, `DB2LTTYP.ctl`) do not hold
//! tell the two orders apart: `A`, whose key is `A ` (41 20), and `A\x01`
//! (41 01). Their right-trimmed text sorts `A` first; their padded bytes sort
//! `A\x01` first, since 01 is below the padding space. `batch mnttrdb2` stores
//! such codes as its `X(2)` input holds them (P-COBTUPDT-007). Every
//! expectation below is the padded order `01` … `07`, `A\x01`, `A `.
//!
//! The test starts its own container through `carddemo_testkit::TestStack`,
//! so it needs a running Docker daemon (AAP 0.10.4). The two codes are
//! inserted through the `TranTypeMasterStore` port, so the test adds no
//! compile-time query.

use carddemo_domain::entities::TranTypeMaster;
use carddemo_domain::messages::{
    NO_MORE_PAGES_FOR_THESE_SEARCH_CONDITIONS, NO_PREVIOUS_PAGES_TO_DISPLAY_COTRTLIC,
};
use carddemo_domain::numeric::{Alnum, TranTypeCode};
use carddemo_domain::ports::{Browse, Direction, TranTypeFilter, TranTypeMasterStore};
use carddemo_domain::rules::tran_type_list::{self, TranTypeListInput};
use carddemo_persistence::Db;
use carddemo_persistence::seed::{self, PreparedSeed, SeedMode, SeedSources};
use carddemo_testkit::{TestStack, block_on, fixtures};

/// The code `A`, stored as `A` and held as the `X(2)` bytes `A `.
const A: [u8; 2] = *b"A ";

/// The code `A` followed by the control byte 01, which sorts below the
/// padding space of [`A`].
const A_CONTROL: [u8; 2] = *b"A\x01";

/// The seven seeded type codes, in key order.
const SEEDED: [[u8; 2]; 7] = [*b"01", *b"02", *b"03", *b"04", *b"05", *b"06", *b"07"];

/// A limit beyond every row, so a page shows the cursor's whole membership.
const ALL: u16 = 20;

/// The fixture seed of the repository's sources, prepared on the calling
/// thread before [`block_on`], outside every runtime, as an adapter prepares
/// the seed on its blocking pool, so the argon2id hashing never runs inside
/// the async phase.
fn fixture_seed() -> PreparedSeed {
    seed::prepare(
        SeedMode::Full,
        &SeedSources::with_defaults(&fixtures::repo_root()),
    )
}

/// The code of the two `X(2)` bytes `bytes`.
fn code(bytes: [u8; 2]) -> TranTypeCode {
    TranTypeCode::from_alnum(Alnum::from_array(bytes))
}

/// The codes `bytes`, in the order given.
fn codes(bytes: &[[u8; 2]]) -> Vec<TranTypeCode> {
    bytes.iter().copied().map(code).collect()
}

/// The seeded codes in ascending key order, followed by `then`.
fn seeded_then(then: &[[u8; 2]]) -> Vec<TranTypeCode> {
    let mut all = codes(&SEEDED);
    all.extend(codes(then));
    all
}

/// The codes `then`, followed by the seeded codes in descending key order.
fn then_seeded_descending(then: &[[u8; 2]]) -> Vec<TranTypeCode> {
    let mut all = codes(then);
    all.extend(SEEDED.iter().rev().copied().map(code));
    all
}

/// The codes of `masters`, in the order given.
fn codes_of(masters: &[TranTypeMaster]) -> Vec<TranTypeCode> {
    masters.iter().map(|master| master.tr_type).collect()
}

/// A fresh migrated database in the seeded state, with the masters [`A`]
/// (`ALPHA`) and [`A_CONTROL`] (`ALPHA CONTROL`) inserted and committed. The
/// stack must outlive the pool's use: dropping it removes the container.
async fn seeded_with_both_codes(prepared: PreparedSeed) -> (TestStack, Db) {
    let stack = TestStack::postgres().await;
    let db = Db::from_pool(stack.pool().clone());
    seed::run(&db, prepared)
        .await
        .expect("the fixture seed loads");
    let mut uow = db.begin().await.expect("a unit of work");
    for (bytes, description) in [(A, "ALPHA"), (A_CONTROL, "ALPHA CONTROL")] {
        let master = TranTypeMaster::try_new(code(bytes), description.to_owned())
            .expect("a CHAR(2) code and a VARCHAR(50) description");
        uow.store()
            .insert_tran_type(master)
            .await
            .unwrap_or_else(|error| panic!("insert {bytes:?}: {error:?}"));
    }
    uow.commit().await.expect("the two masters commit");
    (stack, db)
}

/// One `browse_tran_types` call and the codes it must return, in fetch order.
struct BrowseCase {
    name: &'static str,
    start: Option<[u8; 2]>,
    direction: Direction,
    limit: u16,
    type_code: Option<[u8; 2]>,
    description: Option<&'static str>,
    expected: Vec<TranTypeCode>,
}

impl BrowseCase {
    /// An unfiltered case.
    fn unfiltered(
        name: &'static str,
        start: Option<[u8; 2]>,
        direction: Direction,
        limit: u16,
        expected: Vec<TranTypeCode>,
    ) -> Self {
        Self {
            name,
            start,
            direction,
            limit,
            type_code: None,
            description: None,
            expected,
        }
    }

    /// A case with the type filter `TR_TYPE = type_code`, every row fetched.
    fn of_type(
        name: &'static str,
        type_code: [u8; 2],
        start: [u8; 2],
        direction: Direction,
        expected: Vec<TranTypeCode>,
    ) -> Self {
        Self {
            type_code: Some(type_code),
            ..Self::unfiltered(name, Some(start), direction, ALL, expected)
        }
    }

    /// A case with the description filter `TR_DESCRIPTION LIKE pattern`, from
    /// either end, every row fetched.
    fn described(
        name: &'static str,
        pattern: &'static str,
        direction: Direction,
        expected: Vec<TranTypeCode>,
    ) -> Self {
        Self {
            description: Some(pattern),
            ..Self::unfiltered(name, None, direction, ALL, expected)
        }
    }
}

/// Every `browse_tran_types` case: both directions from either end and from
/// each of the two codes, a limit that ends between them, and the type and
/// description filters on them.
fn browse_cases() -> Vec<BrowseCase> {
    use Direction::{Backward, Forward};
    vec![
        BrowseCase::unfiltered(
            "forward from the first type",
            None,
            Forward,
            ALL,
            seeded_then(&[A_CONTROL, A]),
        ),
        BrowseCase::unfiltered(
            "forward from the first type, ending before A",
            None,
            Forward,
            8,
            seeded_then(&[A_CONTROL]),
        ),
        BrowseCase::unfiltered("forward from A", Some(A), Forward, ALL, codes(&[A])),
        BrowseCase::unfiltered(
            "forward from A\\x01",
            Some(A_CONTROL),
            Forward,
            ALL,
            codes(&[A_CONTROL, A]),
        ),
        BrowseCase::unfiltered(
            "backward from the last type",
            None,
            Backward,
            ALL,
            then_seeded_descending(&[A, A_CONTROL]),
        ),
        BrowseCase::unfiltered(
            "backward strictly below A",
            Some(A),
            Backward,
            ALL,
            then_seeded_descending(&[A_CONTROL]),
        ),
        BrowseCase::unfiltered(
            "backward strictly below A\\x01",
            Some(A_CONTROL),
            Backward,
            ALL,
            then_seeded_descending(&[]),
        ),
        BrowseCase::unfiltered(
            "backward below the blank key, which no code sorts below",
            Some(*b"  "),
            Backward,
            ALL,
            Vec::new(),
        ),
        BrowseCase::of_type(
            "type A forward from A\\x01",
            A,
            A_CONTROL,
            Forward,
            codes(&[A]),
        ),
        BrowseCase::of_type(
            "type A\\x01 forward from A",
            A_CONTROL,
            A,
            Forward,
            Vec::new(),
        ),
        BrowseCase::of_type(
            "type A backward below A\\x01",
            A,
            A_CONTROL,
            Backward,
            Vec::new(),
        ),
        BrowseCase::of_type(
            "type A\\x01 backward below A",
            A_CONTROL,
            A,
            Backward,
            codes(&[A_CONTROL]),
        ),
        BrowseCase::described(
            "description ALPHA forward",
            "%ALPHA%",
            Forward,
            codes(&[A_CONTROL, A]),
        ),
        BrowseCase::described(
            "description ALPHA backward",
            "%ALPHA%",
            Backward,
            codes(&[A, A_CONTROL]),
        ),
    ]
}

/// `browse_tran_types` compares and orders the codes as their padded `X(2)`
/// bytes: each case's page holds exactly the codes its cursor fetches, in
/// fetch order.
#[test]
fn browse_tran_types_follows_the_padded_key_order() {
    let prepared = fixture_seed();
    block_on(async {
        let (_stack, db) = seeded_with_both_codes(prepared).await;
        let mut uow = db.begin().await.expect("a unit of work");
        for case in browse_cases() {
            let browse = Browse {
                start_key: case.start.map(code),
                direction: case.direction,
                limit: case.limit,
            };
            let filter = TranTypeFilter {
                type_code: case.type_code.map(code),
                description: case.description.map(str::to_owned),
            };
            let page = uow
                .store()
                .browse_tran_types(browse, filter)
                .await
                .unwrap_or_else(|error| panic!("{}: {error:?}", case.name));
            assert_eq!(codes_of(&page), case.expected, "{}", case.name);
        }
        uow.rollback()
            .await
            .expect("the read-only unit of work ends");
    });
}

/// One `list` request and the page it must answer.
struct ListCase {
    name: &'static str,
    start: Option<[u8; 2]>,
    direction: Direction,
    page_size: u16,
    items: Vec<TranTypeCode>,
    next_key: Option<[u8; 2]>,
    prev_key: Option<[u8; 2]>,
    message: Option<&'static str>,
}

/// Every `list` case: the first page of the screen's seven rows and of eight,
/// F8 from each of the two codes, and F7 from each of them.
fn list_cases() -> Vec<ListCase> {
    use Direction::{Backward, Forward};
    vec![
        ListCase {
            name: "first page of seven, A\\x01 the look-ahead",
            start: None,
            direction: Forward,
            page_size: tran_type_list::SCREEN_ROWS,
            items: codes(&SEEDED),
            next_key: Some(A_CONTROL),
            prev_key: None,
            message: None,
        },
        ListCase {
            name: "first page of eight, ending at A\\x01",
            start: None,
            direction: Forward,
            page_size: 8,
            items: seeded_then(&[A_CONTROL]),
            next_key: Some(A),
            prev_key: None,
            message: None,
        },
        ListCase {
            name: "F8 from A\\x01",
            start: Some(A_CONTROL),
            direction: Forward,
            page_size: tran_type_list::SCREEN_ROWS,
            items: codes(&[A_CONTROL, A]),
            next_key: None,
            prev_key: Some(A_CONTROL),
            message: Some(NO_MORE_PAGES_FOR_THESE_SEARCH_CONDITIONS),
        },
        ListCase {
            name: "F8 from A",
            start: Some(A),
            direction: Forward,
            page_size: tran_type_list::SCREEN_ROWS,
            items: codes(&[A]),
            next_key: None,
            prev_key: Some(A),
            message: Some(NO_MORE_PAGES_FOR_THESE_SEARCH_CONDITIONS),
        },
        ListCase {
            name: "F7 from A\\x01 reaches the first page",
            start: Some(A_CONTROL),
            direction: Backward,
            page_size: tran_type_list::SCREEN_ROWS,
            items: codes(&SEEDED),
            next_key: Some(A_CONTROL),
            prev_key: None,
            message: Some(NO_PREVIOUS_PAGES_TO_DISPLAY_COTRTLIC),
        },
        ListCase {
            name: "F7 from A shows 02 to A\\x01, 01 before it",
            start: Some(A),
            direction: Backward,
            page_size: tran_type_list::SCREEN_ROWS,
            items: codes(&[*b"02", *b"03", *b"04", *b"05", *b"06", *b"07", A_CONTROL]),
            next_key: Some(A),
            prev_key: Some(*b"02"),
            message: None,
        },
    ]
}

/// `tran_type_list::list` over the store pages in the padded key order: each
/// case's page shows its codes ascending, with the look-ahead next key, the
/// previous-page key and the boundary text the program answers.
#[test]
fn tran_type_list_pages_in_the_padded_key_order() {
    let prepared = fixture_seed();
    block_on(async {
        let (_stack, db) = seeded_with_both_codes(prepared).await;
        let mut uow = db.begin().await.expect("a unit of work");
        for case in list_cases() {
            let input = TranTypeListInput {
                start_key: case.start.map(code),
                direction: case.direction,
                page_size: case.page_size,
                ..TranTypeListInput::default()
            };
            let page = tran_type_list::list(&mut uow.store(), input)
                .await
                .unwrap_or_else(|error| panic!("{}: {error:?}", case.name));
            assert_eq!(codes_of(&page.items), case.items, "{}", case.name);
            assert_eq!(
                (page.next_key, page.prev_key, page.message),
                (
                    case.next_key.map(code),
                    case.prev_key.map(code),
                    case.message
                ),
                "{}: (next_key, prev_key, message)",
                case.name
            );
        }
        uow.rollback()
            .await
            .expect("the read-only unit of work ends");
    });
}
