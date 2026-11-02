//! The card cross-reference: VSAM `CCXREF` (`CVACT03Y` `CARD-XREF-RECORD`)
//! keyed by card number, and its alternate index `CXACAIX` on the account,
//! both over the table `card_xref`. The [`XrefReader`] port on [`PgStore`].
//!
//! # Read-only
//!
//! No carried program writes `CCXREF`. The online programs and the batch jobs
//! only read it, and the `IDCAMS` `REPRO` that loads it
//! [app/jcl/XREFFILE.jcl:L64] is replaced by `crate::seed`, the only writer of
//! the table. Every method here is a read without a lock, each under its own
//! savepoint of the caller's transaction, so a failed read leaves that
//! transaction usable: INTCALC reads the cross-reference of an account after
//! rewriting the previous one (P-CBACT04C-010).
//!
//! # Key order
//!
//! The KSDS key `XREF-CARD-NUM X(16)` (`KEYS(16 0)` [app/jcl/XREFFILE.jcl:L43],
//! P-CVACT03Y-001) is stored right-trimmed in `xref_card_num VARCHAR(16)
//! COLLATE "C"` and bound right-trimmed through [`CardNumber::trimmed`]. The
//! `"C"` collation compares bytes, so the column order is the ascending key
//! order of the cluster for every key without a byte below the space, which
//! holds for every card number of the fixture corpus. The statements repeat
//! `COLLATE "C"` so the order holds whatever collation the column carries.
//!
//! # The alternate index `CXACAIX`
//!
//! `KEYS(11,25) NONUNIQUEKEY` on `XREF-ACCT-ID` [app/jcl/XREFFILE.jcl:L72-76],
//! built by `BLDINDEX` over the loaded cluster [app/jcl/XREFFILE.jcl:L100-102],
//! lists the base keys of each account in ascending order. A `READ` through the
//! path, the CICS file `CXACAIX` [app/csd/CARDDEMO.CSD:L63-65] or the
//! `ALTERNATE RECORD KEY` of `CBACT04C` [app/cbl/CBACT04C.cbl:L34-39], returns
//! the first of them: here the row of the account with the lowest card number,
//! found through the index `cxacaix`.

use carddemo_domain::entities::CardXref;
use carddemo_domain::numeric::{AccountId, CardNumber};
use carddemo_domain::ports::{PortError, PortResult, XrefReader};

use super::{account_id, card_number, customer_id, limit_i64};
use crate::PersistenceError;
use crate::tx::{PgStore, savepoint, settle};

/// The table every stored-value misfit names.
const TABLE: &str = "card_xref";

/// The byte of COBOL `LOW-VALUES`. No stored key holds it: the `card_xref`
/// check admits only `\x01` to `\xff`, and `PostgreSQL` text cannot carry it.
const LOW_VALUE: u8 = 0x00;

/// One `card_xref` row as the statements read it; every column is `NOT NULL`.
///
/// The statements alias the columns to these names, which drop the `xref_`
/// prefix every column shares; conversion errors name the real columns.
struct XrefRow {
    card_num: String,
    cust_id: i32,
    acct_id: i64,
}

impl TryFrom<XrefRow> for CardXref {
    type Error = PersistenceError;

    /// Holds each column to its PIC clause: `XREF-CARD-NUM X(16)`,
    /// `XREF-CUST-ID 9(09)` and `XREF-ACCT-ID 9(11)` (P-CVACT03Y-001 to
    /// P-CVACT03Y-003). A misfit is [`PersistenceError::Corrupt`] naming the
    /// column, never its value.
    fn try_from(row: XrefRow) -> Result<Self, Self::Error> {
        Ok(Self {
            xref_card_num: card_number(&row.card_num, TABLE, "xref_card_num")?,
            xref_cust_id: customer_id(row.cust_id, TABLE, "xref_cust_id")?,
            xref_acct_id: account_id(row.acct_id, TABLE, "xref_acct_id")?,
        })
    }
}

/// The record of a keyed read: `NOTFND` when no row came back.
fn found(row: Option<XrefRow>) -> PortResult<CardXref> {
    let row = row.ok_or(PortError::NotFound)?;
    CardXref::try_from(row).map_err(PortError::from)
}

/// The records of a sweep, in the order the statement returned them.
fn records(rows: Vec<XrefRow>) -> PortResult<Vec<CardXref>> {
    rows.into_iter()
        .map(CardXref::try_from)
        .collect::<Result<Vec<_>, _>>()
        .map_err(PortError::from)
}

impl XrefReader for PgStore<'_, '_> {
    /// CBTRN02C 1500-A-LOOKUP-XREF — P-CBTRN02C-008
    /// [app/cbl/CBTRN02C.cbl:L380-391]; CBTRN03C 1500-A-LOOKUP-XREF —
    /// P-CBTRN03C-005 [app/cbl/CBTRN03C.cbl:L484-492]; COTRN02C
    /// READ-CCXREF-FILE — P-COTRN02C-009 [app/cbl/COTRN02C.cbl:L609-637];
    /// COPAUA0C 5100-READ-XREF-RECORD — P-COPAUA0C-006
    /// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L472-514].
    ///
    /// The keyed `READ` of `CCXREF` by `XREF-CARD-NUM`. A key holding a
    /// `LOW-VALUES` byte, such as the card field of a low-values daily
    /// transaction record, is `NOTFND` without a statement: no stored key holds
    /// that byte, and binding it would fail the statement with a server error
    /// instead of answering `NOTFND` as the COBOL does.
    async fn read_xref_by_card(&mut self, card: CardNumber) -> PortResult<CardXref> {
        if card.as_bytes().contains(&LOW_VALUE) {
            return Err(PortError::NotFound);
        }
        let key = card.trimmed();
        let mut sp = savepoint(self.tx).await?;
        let result = sqlx::query_as!(
            XrefRow,
            r#"SELECT xref_card_num AS card_num,
                      xref_cust_id  AS cust_id,
                      xref_acct_id  AS acct_id
                 FROM card_xref
                WHERE xref_card_num = $1"#,
            &*key
        )
        .fetch_optional(&mut *sp)
        .await;
        found(settle(sp, result).await?)
    }

    /// COACTVWC 9200-GETCARDXREF-BYACCT — P-COACTVWC-007, P-COACTVWC-008
    /// [app/cbl/COACTVWC.cbl:L723-771]; COACTUPC 9200-GETCARDXREF-BYACCT —
    /// P-COACTUPC-043 [app/cbl/COACTUPC.cbl:L3650-3697]; COBIL00C
    /// READ-CXACAIX-FILE — P-COBIL00C-011 [app/cbl/COBIL00C.cbl:L408-436];
    /// COTRN02C READ-CXACAIX-FILE — P-COTRN02C-008
    /// [app/cbl/COTRN02C.cbl:L576-604]; CBACT04C 1110-GET-XREF-DATA —
    /// P-CBACT04C-002 [app/cbl/CBACT04C.cbl:L393-413]; COPAUS0C
    /// GETCARDXREF-BYACCT — P-COPAUS0C-007
    /// [app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L812-862].
    ///
    /// The `READ` through `CXACAIX` by the 11-digit account key: the first base
    /// record of the account in card-number order. The `9(11)` key has a fixed
    /// width, so the `BIGINT` equality matches exactly the records the path's
    /// key comparison matches.
    ///
    /// Only the record is returned. When a further cross-reference shares the
    /// account, the COBOL `READ` also reports CICS `DUPKEY` (file status `02`
    /// in `CBACT04C`), which every caller routes to its error branch; the port
    /// declares no such outcome, so it is not reproduced. The input class
    /// cannot arise: the table is written only by the seed from
    /// `cardxref.txt`, where each account has exactly one cross-reference
    /// (P-CVACT03Y-006).
    async fn read_xref_by_account(&mut self, id: AccountId) -> PortResult<CardXref> {
        let mut sp = savepoint(self.tx).await?;
        let result = sqlx::query_as!(
            XrefRow,
            r#"SELECT xref_card_num AS card_num,
                      xref_cust_id  AS cust_id,
                      xref_acct_id  AS acct_id
                 FROM card_xref
                WHERE xref_acct_id = $1
                ORDER BY xref_card_num COLLATE "C"
                LIMIT 1"#,
            id.to_i64()
        )
        .fetch_optional(&mut *sp)
        .await;
        found(settle(sp, result).await?)
    }

    /// CBSTM03B 2000-XREFFILE-PROC — P-CBSTM03B-003
    /// [app/cbl/CBSTM03B.CBL:L157-179], driving CBSTM03A
    /// 1000-XREFFILE-GET-NEXT — P-CBSTM03A-1257
    /// [app/cbl/CBSTM03A.CBL:L345-366].
    ///
    /// The sequential `READ` of `CCXREF`, one page at a time: up to `limit`
    /// records whose card number follows `after`, or from the first record
    /// when `after` is `None`. The two cases are separate statements, so each
    /// page is a range scan of the primary key rather than a filtered scan from
    /// the start of the file.
    async fn xrefs_after(
        &mut self,
        after: Option<CardNumber>,
        limit: u16,
    ) -> PortResult<Vec<CardXref>> {
        let limit = limit_i64(limit)?;
        let mut sp = savepoint(self.tx).await?;
        let result = match after {
            None => {
                sqlx::query_as!(
                    XrefRow,
                    r#"SELECT xref_card_num AS card_num,
                              xref_cust_id  AS cust_id,
                              xref_acct_id  AS acct_id
                         FROM card_xref
                        ORDER BY xref_card_num COLLATE "C"
                        LIMIT $1"#,
                    limit
                )
                .fetch_all(&mut *sp)
                .await
            }
            Some(card) => {
                let key = card.trimmed();
                sqlx::query_as!(
                    XrefRow,
                    r#"SELECT xref_card_num AS card_num,
                              xref_cust_id  AS cust_id,
                              xref_acct_id  AS acct_id
                         FROM card_xref
                        WHERE xref_card_num COLLATE "C" > $1
                        ORDER BY xref_card_num COLLATE "C"
                        LIMIT $2"#,
                    &*key,
                    limit
                )
                .fetch_all(&mut *sp)
                .await
            }
        };
        records(settle(sp, result).await?)
    }
}

#[cfg(test)]
mod tests {
    use carddemo_domain::entities::CardXref;
    use carddemo_domain::numeric::{AccountId, CardNumber, CustomerId, NumericError};
    use carddemo_domain::ports::PortError;

    use super::{TABLE, XrefRow, found, records};
    use crate::PersistenceError;

    /// The first `cardxref.txt` record as its row (P-CVACT03Y-006).
    fn first_fixture_row() -> XrefRow {
        XrefRow {
            card_num: "0500024453765740".to_owned(),
            cust_id: 50,
            acct_id: 50,
        }
    }

    #[test]
    fn a_row_becomes_its_record() -> Result<(), NumericError> {
        let expected = CardXref {
            xref_card_num: CardNumber::from_trimmed("0500024453765740")?,
            xref_cust_id: CustomerId::new(50)?,
            xref_acct_id: AccountId::new(50)?,
        };
        assert_eq!(found(Some(first_fixture_row())), Ok(expected.clone()));
        assert_eq!(records(vec![first_fixture_row()]), Ok(vec![expected]));
        Ok(())
    }

    #[test]
    fn no_row_is_notfnd_and_no_rows_is_the_end_of_the_file() {
        assert_eq!(found(None), Err(PortError::NotFound));
        assert_eq!(records(Vec::new()), Ok(Vec::new()));
    }

    #[test]
    fn a_misfit_column_is_corrupt_naming_the_column_not_the_value() {
        let cases = [
            (
                "xref_card_num",
                XrefRow {
                    card_num: "05000244537657401".to_owned(),
                    ..first_fixture_row()
                },
            ),
            (
                "xref_cust_id",
                XrefRow {
                    cust_id: -50,
                    ..first_fixture_row()
                },
            ),
            (
                "xref_acct_id",
                XrefRow {
                    acct_id: 100_000_000_050,
                    ..first_fixture_row()
                },
            ),
        ];
        for (column, row) in cases {
            match CardXref::try_from(row) {
                Err(PersistenceError::Corrupt {
                    table,
                    column: named,
                    reason,
                }) => {
                    assert_eq!((table, named), (TABLE, column));
                    assert!(!reason.contains("050"), "{column}: {reason}");
                }
                other => panic!("{column}: expected a Corrupt error, got {other:?}"),
            }
        }
    }

    #[test]
    fn a_misfit_reaches_the_port_as_unavailable() {
        let row = XrefRow {
            acct_id: -1,
            ..first_fixture_row()
        };
        assert!(
            matches!(found(Some(row)), Err(PortError::Unavailable(_))),
            "a negative account ID was accepted"
        );
    }
}
