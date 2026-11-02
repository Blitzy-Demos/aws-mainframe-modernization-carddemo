//! The card master: VSAM `CARDDAT` (`CVACT02Y` `CARD-RECORD`) and its alternate
//! index path `CARDAIX` on `CARD-ACCT-ID`, held in the one table `cards`.
//!
//! [`CardReader`] and [`CardWriter`] on [`PgStore`], the accesses `COCRDLIC`,
//! `COCRDSLC` and `COCRDUPC` issue on the file
//! [app/csd/CARDDEMO.CSD:L13] [app/csd/CARDDEMO.CSD:L25]:
//!
//! | Port method | COBOL access | SQL |
//! |---|---|---|
//! | `read_card` | `READ CARDDAT` by `CARD-NUM` | `WHERE card_number = $1` |
//! | `read_card_for_update` | `READ CARDDAT UPDATE` | the same, `FOR UPDATE`, under a savepoint |
//! | `browse_cards` | `STARTBR GTEQ` with `READNEXT` or `READPREV` | key range, filter, `ORDER BY`, `LIMIT` |
//! | `cards_for_account` | `READ CARDAIX` by `CARD-ACCT-ID` | `WHERE card_acct_id = $1 ORDER BY card_number` |
//! | `rewrite_card` | `REWRITE CARDDAT` | `UPDATE` of every non-key column, under a savepoint |
//!
//! Every statement, the reads included, runs under its own savepoint through
//! `crate::tx::savepoint` and `crate::tx::settle`, so a failed one leaves the
//! caller's transaction usable.
//!
//! # Key order
//!
//! `card_number` is `VARCHAR(16) COLLATE "C"` (`migrations/0002_master_data.sql`),
//! so comparisons and `ORDER BY` follow byte order, the order of the KSDS key
//! `KEYS(16 0)` [app/jcl/CARDFILE.jcl:L54]. Keys are bound right-trimmed, as
//! they are stored. Byte order of the trimmed text equals the order of the
//! space-padded `X(16)` keys whenever no key holds a character below the
//! space; every card number is 16 digits loaded from `carddata.txt`, and no
//! program changes a card number.
//!
//! The non-unique alternate index `KEYS(11 16) NONUNIQUEKEY`
//! [app/jcl/CARDFILE.jcl:L83-86] is the index `cardaix`. `BLDINDEX` keeps the
//! base keys of one alternate key in ascending order, so the first card of an
//! account in card-number order is the record a `READ` on the path returns.

use carddemo_domain::entities::Card;
use carddemo_domain::numeric::{AccountId, CardNumber};
use carddemo_domain::ports::{
    Browse, CardFilter, CardReader, CardWriter, Direction, PortError, PortResult,
};

use super::{account_id, alnum, card_number, cvv, expect_affected, limit_i64};
use crate::PersistenceError;
use crate::tx::{PgStore, savepoint, settle};

/// The table every column below is read from, as
/// [`PersistenceError::Corrupt`] names it.
const TABLE: &str = "cards";

/// One `cards` row as `PostgreSQL` returns it, before each column is held to
/// its PIC clause by the conversion into [`Card`].
///
/// Every query selects the columns under these names, the column name without
/// its `card_` prefix: a private struct whose fields all share one prefix
/// fails clippy's `struct_field_names`.
struct CardRow {
    num: String,
    acct_id: i64,
    cvv_cd: i32,
    embossed_name: String,
    expiraion_date: String,
    active_status: String,
}

impl TryFrom<CardRow> for Card {
    type Error = PersistenceError;

    /// `CVACT02Y` `CARD-RECORD` from its row, field by field as section A
    /// fixes them: `P-CVACT02Y-001` to `P-CVACT02Y-006`. The `FILLER X(59)`
    /// (`P-CVACT02Y-007`) is not stored.
    fn try_from(row: CardRow) -> Result<Self, Self::Error> {
        Ok(Self {
            card_num: card_number(&row.num, TABLE, "card_number")?,
            card_acct_id: account_id(row.acct_id, TABLE, "card_acct_id")?,
            card_cvv_cd: cvv(row.cvv_cd, TABLE, "card_cvv_cd")?,
            card_embossed_name: alnum(&row.embossed_name, TABLE, "card_embossed_name")?,
            card_expiraion_date: alnum(&row.expiraion_date, TABLE, "card_expiraion_date")?,
            card_active_status: alnum(&row.active_status, TABLE, "card_active_status")?,
        })
    }
}

/// The rows of a query in read order, each converted into a [`Card`].
fn cards_of(rows: Vec<CardRow>) -> PortResult<Vec<Card>> {
    rows.into_iter()
        .map(|row| Card::try_from(row).map_err(PortError::from))
        .collect()
}

impl CardReader for PgStore<'_, '_> {
    /// `COCRDSLC` `9100-GETCARD-BYACCTCARD` — P-COCRDSLC-004
    /// [app/cbl/COCRDSLC.cbl:L736-750]; `COCRDUPC` `9100-GETCARD-BYACCTCARD`
    /// — P-COCRDUPC-023 [app/cbl/COCRDUPC.cbl:L1376-1390]. Keyed by the card
    /// number alone: both programs leave the account out of the key (E-55).
    async fn read_card(&mut self, card: CardNumber) -> PortResult<Card> {
        let key = card.trimmed();
        let mut sp = savepoint(self.tx).await?;
        let result = sqlx::query_as!(
            CardRow,
            "SELECT card_number AS num, card_acct_id AS acct_id, card_cvv_cd AS cvv_cd,
                    card_embossed_name AS embossed_name,
                    card_expiraion_date AS expiraion_date,
                    card_active_status AS active_status
               FROM cards
              WHERE card_number = $1",
            key.as_ref()
        )
        .fetch_one(&mut *sp)
        .await;
        let row = settle(sp, result).await?;
        Card::try_from(row).map_err(PortError::from)
    }

    /// `COCRDUPC` `9200-WRITE-PROCESSING` `READ … UPDATE` — P-COCRDUPC-004
    /// [app/cbl/COCRDUPC.cbl:L1420-1436]. The row lock lasts until the caller's
    /// transaction ends, as the CICS update lock lasts until the `REWRITE` or
    /// the task's end. A lock held elsewhere is waited for, as CICS waits; a
    /// lock timeout or deadlock is `PortError::Contention`.
    async fn read_card_for_update(&mut self, card: CardNumber) -> PortResult<Card> {
        let key = card.trimmed();
        let mut sp = savepoint(self.tx).await?;
        let result = sqlx::query_as!(
            CardRow,
            "SELECT card_number AS num, card_acct_id AS acct_id, card_cvv_cd AS cvv_cd,
                    card_embossed_name AS embossed_name,
                    card_expiraion_date AS expiraion_date,
                    card_active_status AS active_status
               FROM cards
              WHERE card_number = $1
                FOR UPDATE",
            key.as_ref()
        )
        .fetch_one(&mut *sp)
        .await;
        let row = settle(sp, result).await?;
        Card::try_from(row).map_err(PortError::from)
    }

    /// `COCRDLIC` `9000-READ-FORWARD` — P-COCRDLIC-003
    /// [app/cbl/COCRDLIC.cbl:L1123-1160]; `9100-READ-BACKWARDS` —
    /// P-COCRDLIC-013 [app/cbl/COCRDLIC.cbl:L1264-1340];
    /// `9500-FILTER-RECORDS` — P-COCRDLIC-004 [app/cbl/COCRDLIC.cbl:L1382-1406].
    ///
    /// `STARTBR GTEQ` on `CARD-NUM`, then `READNEXT` (keys from the start key
    /// up, ascending) or `READPREV` (keys up to the start key, descending). A
    /// `None` start key is `LOW-VALUES` forward and `HIGH-VALUES` backward.
    /// The filter is part of the statement, so an excluded card is skipped and
    /// never counted against the limit, as `9500-FILTER-RECORDS` excludes a
    /// record before `WS-SCRN-COUNTER` is incremented.
    ///
    /// Each direction has four static statements, one per row below, chosen by
    /// which inputs are present (`cards_forward`, `cards_backward`):
    ///
    /// | Card filter | Account filter | Start key | Read through |
    /// |---|---|---|---|
    /// | given | either | either | `cards_pkey`, `card_number = card` |
    /// | absent | given | either | `cardaix`, `card_acct_id = account` |
    /// | absent | absent | given | `cards_pkey`, the range from the start key |
    /// | absent | absent | absent | `cards_pkey`, from the first or last key |
    ///
    /// The statements are static because a nullable-parameter predicate such
    /// as `$1::text IS NULL OR card_number >= $1` is folded away only in a
    /// custom plan. Under the generic plan `PostgreSQL` may adopt for the
    /// prepared statement, it stays an `OR` that no index condition can use, so
    /// a deep page would read every preceding key and the account filter would
    /// not reach `cardaix`. The filters left as `$n IS NULL OR …` (the start
    /// key and the account inside a single-card read, the start key inside one
    /// account's cards) only test the rows the index has already selected: at
    /// most one card, or the cards of one account, which `cardaix` holds on
    /// `card_acct_id` alone, so a direct start bound could not narrow its scan.
    async fn browse_cards(
        &mut self,
        browse: Browse<CardNumber>,
        filter: CardFilter,
    ) -> PortResult<Vec<Card>> {
        let start = browse.start_key.as_ref().map(CardNumber::trimmed);
        let account = filter.account_id.map(AccountId::to_i64);
        let only_card = filter.card_number.as_ref().map(CardNumber::trimmed);
        let limit = limit_i64(browse.limit)?;
        let (only_card, start) = (only_card.as_deref(), start.as_deref());
        let mut sp = savepoint(self.tx).await?;
        let result = match browse.direction {
            Direction::Forward => cards_forward(&mut sp, only_card, account, start, limit).await,
            Direction::Backward => cards_backward(&mut sp, only_card, account, start, limit).await,
        };
        cards_of(settle(sp, result).await?)
    }

    /// `COCRDSLC` `9150-GETCARD-BYACCT` — P-COCRDSLC-017
    /// [app/cbl/COCRDSLC.cbl:L779-790], the `READ` through the path `CARDAIX`.
    /// The paragraph itself is never performed (P-COCRDSLC-021); the port
    /// serves the account's cards in base-key order, the first of them being
    /// the record that `READ` returns.
    async fn cards_for_account(&mut self, id: AccountId) -> PortResult<Vec<Card>> {
        let mut sp = savepoint(self.tx).await?;
        let result = sqlx::query_as!(
            CardRow,
            "SELECT card_number AS num, card_acct_id AS acct_id, card_cvv_cd AS cvv_cd,
                    card_embossed_name AS embossed_name,
                    card_expiraion_date AS expiraion_date,
                    card_active_status AS active_status
               FROM cards
              WHERE card_acct_id = $1
              ORDER BY card_number ASC",
            id.to_i64()
        )
        .fetch_all(&mut *sp)
        .await;
        cards_of(settle(sp, result).await?)
    }
}

/// The rows of a [`Direction::Forward`] [`CardReader::browse_cards`] page:
/// `COCRDLIC` `9000-READ-FORWARD` — P-COCRDLIC-003
/// [app/cbl/COCRDLIC.cbl:L1123-1160], with `9500-FILTER-RECORDS` —
/// P-COCRDLIC-004 [app/cbl/COCRDLIC.cbl:L1382-1406]. Up to `limit` cards in
/// ascending key order from `start` inclusive, or from the first key, read
/// on `conn` by the static statement of the inputs present.
async fn cards_forward(
    conn: &mut sqlx::PgConnection,
    card: Option<&str>,
    account: Option<i64>,
    start: Option<&str>,
    limit: i64,
) -> Result<Vec<CardRow>, sqlx::Error> {
    match (card, account, start) {
        (Some(card), account, start) => {
            sqlx::query_as!(
                CardRow,
                "SELECT card_number AS num, card_acct_id AS acct_id,
                        card_cvv_cd AS cvv_cd, card_embossed_name AS embossed_name,
                        card_expiraion_date AS expiraion_date,
                        card_active_status AS active_status
                   FROM cards
                  WHERE card_number = $1
                    AND ($2::text IS NULL OR card_number >= $2)
                    AND ($3::bigint IS NULL OR card_acct_id = $3)
                  ORDER BY card_number ASC
                  LIMIT $4",
                card,
                start,
                account,
                limit
            )
            .fetch_all(&mut *conn)
            .await
        }
        (None, Some(account), start) => {
            sqlx::query_as!(
                CardRow,
                "SELECT card_number AS num, card_acct_id AS acct_id,
                        card_cvv_cd AS cvv_cd, card_embossed_name AS embossed_name,
                        card_expiraion_date AS expiraion_date,
                        card_active_status AS active_status
                   FROM cards
                  WHERE card_acct_id = $1
                    AND ($2::text IS NULL OR card_number >= $2)
                  ORDER BY card_number ASC
                  LIMIT $3",
                account,
                start,
                limit
            )
            .fetch_all(&mut *conn)
            .await
        }
        (None, None, Some(start)) => {
            sqlx::query_as!(
                CardRow,
                "SELECT card_number AS num, card_acct_id AS acct_id,
                        card_cvv_cd AS cvv_cd, card_embossed_name AS embossed_name,
                        card_expiraion_date AS expiraion_date,
                        card_active_status AS active_status
                   FROM cards
                  WHERE card_number >= $1
                  ORDER BY card_number ASC
                  LIMIT $2",
                start,
                limit
            )
            .fetch_all(&mut *conn)
            .await
        }
        (None, None, None) => {
            sqlx::query_as!(
                CardRow,
                "SELECT card_number AS num, card_acct_id AS acct_id,
                        card_cvv_cd AS cvv_cd, card_embossed_name AS embossed_name,
                        card_expiraion_date AS expiraion_date,
                        card_active_status AS active_status
                   FROM cards
                  ORDER BY card_number ASC
                  LIMIT $1",
                limit
            )
            .fetch_all(&mut *conn)
            .await
        }
    }
}

/// The rows of a [`Direction::Backward`] [`CardReader::browse_cards`] page:
/// `COCRDLIC` `9100-READ-BACKWARDS` — P-COCRDLIC-013
/// [app/cbl/COCRDLIC.cbl:L1264-1340], with `9500-FILTER-RECORDS` —
/// P-COCRDLIC-004 [app/cbl/COCRDLIC.cbl:L1382-1406]. Up to `limit` cards in
/// descending key order from `start` inclusive, or from the last key, read
/// on `conn` by the static statement of the inputs present.
async fn cards_backward(
    conn: &mut sqlx::PgConnection,
    card: Option<&str>,
    account: Option<i64>,
    start: Option<&str>,
    limit: i64,
) -> Result<Vec<CardRow>, sqlx::Error> {
    match (card, account, start) {
        (Some(card), account, start) => {
            sqlx::query_as!(
                CardRow,
                "SELECT card_number AS num, card_acct_id AS acct_id,
                        card_cvv_cd AS cvv_cd, card_embossed_name AS embossed_name,
                        card_expiraion_date AS expiraion_date,
                        card_active_status AS active_status
                   FROM cards
                  WHERE card_number = $1
                    AND ($2::text IS NULL OR card_number <= $2)
                    AND ($3::bigint IS NULL OR card_acct_id = $3)
                  ORDER BY card_number DESC
                  LIMIT $4",
                card,
                start,
                account,
                limit
            )
            .fetch_all(&mut *conn)
            .await
        }
        (None, Some(account), start) => {
            sqlx::query_as!(
                CardRow,
                "SELECT card_number AS num, card_acct_id AS acct_id,
                        card_cvv_cd AS cvv_cd, card_embossed_name AS embossed_name,
                        card_expiraion_date AS expiraion_date,
                        card_active_status AS active_status
                   FROM cards
                  WHERE card_acct_id = $1
                    AND ($2::text IS NULL OR card_number <= $2)
                  ORDER BY card_number DESC
                  LIMIT $3",
                account,
                start,
                limit
            )
            .fetch_all(&mut *conn)
            .await
        }
        (None, None, Some(start)) => {
            sqlx::query_as!(
                CardRow,
                "SELECT card_number AS num, card_acct_id AS acct_id,
                        card_cvv_cd AS cvv_cd, card_embossed_name AS embossed_name,
                        card_expiraion_date AS expiraion_date,
                        card_active_status AS active_status
                   FROM cards
                  WHERE card_number <= $1
                  ORDER BY card_number DESC
                  LIMIT $2",
                start,
                limit
            )
            .fetch_all(&mut *conn)
            .await
        }
        (None, None, None) => {
            sqlx::query_as!(
                CardRow,
                "SELECT card_number AS num, card_acct_id AS acct_id,
                        card_cvv_cd AS cvv_cd, card_embossed_name AS embossed_name,
                        card_expiraion_date AS expiraion_date,
                        card_active_status AS active_status
                   FROM cards
                  ORDER BY card_number DESC
                  LIMIT $1",
                limit
            )
            .fetch_all(&mut *conn)
            .await
        }
    }
}

impl CardWriter for PgStore<'_, '_> {
    /// `COCRDUPC` `9200-WRITE-PROCESSING` `REWRITE CARDDAT` — P-COCRDUPC-006
    /// [app/cbl/COCRDUPC.cbl:L1458-1491]. The whole record is rewritten, the
    /// account included: `CARD-UPDATE-ACCT-ID` comes from the screen, not from
    /// the record read (E-55). The card number is the key and is not changed.
    async fn rewrite_card(&mut self, card: Card) -> PortResult<()> {
        let key = card.card_num.trimmed();
        let embossed_name = card.card_embossed_name.trimmed();
        let expiraion_date = card.card_expiraion_date.trimmed();
        let active_status = card.card_active_status.trimmed();
        let mut sp = savepoint(self.tx).await?;
        let result = sqlx::query!(
            "UPDATE cards
                SET card_acct_id = $2,
                    card_cvv_cd = $3,
                    card_embossed_name = $4,
                    card_expiraion_date = $5,
                    card_active_status = $6
              WHERE card_number = $1",
            key.as_ref(),
            card.card_acct_id.to_i64(),
            i32::from(card.card_cvv_cd.get()),
            embossed_name.as_ref(),
            expiraion_date.as_ref(),
            active_status.as_ref()
        )
        .execute(&mut *sp)
        .await;
        let done = settle(sp, result).await?;
        expect_affected(done.rows_affected())
    }
}

#[cfg(test)]
mod tests {
    use carddemo_domain::entities::Card;
    use carddemo_domain::numeric::{AccountId, Alnum, CardNumber, Cvv, NumericError};
    use carddemo_domain::ports::PortError;

    use super::CardRow;
    use crate::PersistenceError;

    /// The first fixture card as its row holds it, right-trimmed
    /// (`P-CVACT02Y-009`) [app/data/ASCII/carddata.txt:L1], with an arbitrary
    /// verification code.
    fn fixture_row() -> CardRow {
        CardRow {
            num: "0500024453765740".to_owned(),
            acct_id: 50,
            cvv_cd: 123,
            embossed_name: "Aniya Von".to_owned(),
            expiraion_date: "2023-03-09".to_owned(),
            active_status: "Y".to_owned(),
        }
    }

    #[test]
    fn a_stored_row_becomes_the_padded_record() -> Result<(), NumericError> {
        let card = Card::try_from(fixture_row());
        let expected = Card {
            card_num: CardNumber::from_trimmed("0500024453765740")?,
            card_acct_id: AccountId::new(50)?,
            card_cvv_cd: Cvv::new(123)?,
            card_embossed_name: Alnum::move_from("Aniya Von"),
            card_expiraion_date: Alnum::move_from("2023-03-09"),
            card_active_status: Alnum::move_from("Y"),
        };
        assert!(
            matches!(&card, Ok(stored) if *stored == expected),
            "{card:?}"
        );
        let name = card.map(|stored| stored.card_embossed_name.as_bytes().len());
        assert!(matches!(name, Ok(50)), "{name:?}");
        Ok(())
    }

    #[test]
    fn a_misfit_column_is_corrupt_and_reaches_the_port_as_unavailable() {
        let cases = [
            (
                "card_number",
                CardRow {
                    num: "05000244537657401".to_owned(),
                    ..fixture_row()
                },
            ),
            (
                "card_acct_id",
                CardRow {
                    acct_id: 100_000_000_000,
                    ..fixture_row()
                },
            ),
            (
                "card_cvv_cd",
                CardRow {
                    cvv_cd: -1,
                    ..fixture_row()
                },
            ),
            (
                "card_embossed_name",
                CardRow {
                    embossed_name: "N".repeat(51),
                    ..fixture_row()
                },
            ),
            (
                "card_expiraion_date",
                CardRow {
                    expiraion_date: "2023-03-09T".to_owned(),
                    ..fixture_row()
                },
            ),
            (
                "card_active_status",
                CardRow {
                    active_status: "\u{100}".to_owned(),
                    ..fixture_row()
                },
            ),
        ];
        for (column, row) in cases {
            let error = match Card::try_from(row) {
                Err(error) => error,
                Ok(card) => panic!("{column}: a misfit was accepted: {card:?}"),
            };
            assert!(
                matches!(&error, PersistenceError::Corrupt { table: "cards", column: named, .. } if *named == column),
                "{column}: {error:?}"
            );
            let port = PortError::from(error);
            assert!(
                matches!(&port, PortError::Unavailable(text) if text.contains(column)),
                "{column}: {port:?}"
            );
        }
    }
}
