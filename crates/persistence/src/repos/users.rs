//! [`UserStore`] on [`PgStore`]: the user security file, VSAM `USRSEC`
//! (`CSUSR01Y` `SEC-USER-DATA` [app/cpy/CSUSR01Y.cpy:L17-23]), table `users`
//! of `migrations/0002_master_data.sql`.
//!
//! | Field | Audit row | Column | Read back through |
//! |---|---|---|---|
//! | `SEC-USR-ID X(08)` | P-CSUSR01Y-001 | `user_id` | `repos::user_id` |
//! | `SEC-USR-FNAME X(20)` | P-CSUSR01Y-002 | `sec_usr_fname` | `repos::alnum::<20>` |
//! | `SEC-USR-LNAME X(20)` | P-CSUSR01Y-003 | `sec_usr_lname` | `repos::alnum::<20>` |
//! | `SEC-USR-PWD X(08)` | P-CSUSR01Y-004 | `password_hash` | verbatim |
//! | `SEC-USR-TYPE X(01)` | P-CSUSR01Y-005 | `sec_usr_type` | `repos::alnum::<1>` |
//!
//! `SEC-USR-FILLER` (P-CSUSR01Y-006) is not stored.
//!
//! # Access paths
//!
//! | Method | COBOL access | SQL | Savepoint |
//! |---|---|---|---|
//! | `read_user` | `READ` (P-COSGN00C-008, P-COUSR03C-003) | `WHERE user_id = $1` | no |
//! | `read_user_for_update` | `READ … UPDATE` (P-COUSR02C-002, P-COUSR02C-003, P-COUSR03C-004) | `… FOR UPDATE` | yes |
//! | `browse_users` | `STARTBR`, `READNEXT`, `READPREV` (P-COUSR00C-003, P-COUSR00C-009) | padded-key bound, `ORDER BY`, `LIMIT` | no |
//! | `insert_user` | `WRITE` (P-COUSR01C-004, P-COUSR01C-005) | `INSERT`; SQLSTATE 23505 is `Duplicate` | yes |
//! | `update_user` | `REWRITE` (P-COUSR02C-006) | `UPDATE … WHERE user_id = $1`; no row is `NotFound` | yes |
//! | `delete_user` | `DELETE` (P-COUSR03C-005) | `DELETE … WHERE user_id = $1`; no row is `NotFound` | yes |
//!
//! # Keys
//!
//! A [`UserId`] is bound as stored: right-trimmed, case kept. The `user_id`
//! column is `COLLATE "C"`, so the key is compared byte for byte and a
//! lower-case stored ID is not found by the upper-cased ID a sign-on reads
//! with, as in the COBOL (E-20). The sign-on rule upper-cases before it calls
//! the store; the store never converts a key.
//!
//! A key holding a NUL byte names no stored user, because `PostgreSQL` text
//! cannot hold NUL. Binding one would fail on the server and abort the
//! transaction of a read that runs without a savepoint, so the keyed methods
//! answer `NotFound` for it without a query, and a browse cannot position on
//! it.
//!
//! # Credentials
//!
//! The store never hashes, verifies, upper-cases or pads a password: that is
//! `carddemo_domain::auth::password` and the rules (E-20, E-21).
//! `password_hash` is written and read back verbatim. The module logs nothing,
//! so no hash, password or row reaches a log line, and the private row type
//! has no `Debug`.

use std::borrow::Cow;

use carddemo_domain::entities::UserSecurity;
use carddemo_domain::numeric::UserId;
use carddemo_domain::ports::{Browse, Direction, PortError, PortResult, UserStore};

use super::{alnum, expect_affected, limit_i64, user_id};
use crate::tx::{PgStore, savepoint, settle};
use crate::{PersistenceError, port_error};

/// The table every statement here reads or writes.
const TABLE: &str = "users";

/// The COBOL figurative constant `LOW-VALUE`, which `PostgreSQL` text cannot
/// hold.
const LOW_VALUE: u8 = 0x00;

/// One `users` row as `query_as!` reads it: the stored, right-trimmed text of
/// each column.
///
/// No `Debug`: the row carries the password hash.
struct UserRow {
    user_id: String,
    sec_usr_fname: String,
    sec_usr_lname: String,
    password_hash: String,
    sec_usr_type: String,
}

impl TryFrom<UserRow> for UserSecurity {
    type Error = PersistenceError;

    /// The `CSUSR01Y` record of a stored row, each text column padded back to
    /// its `PIC X(n)` width; the hash is kept as stored.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Corrupt`] naming the column when a value does not
    /// fit its field: more characters than the PIC width, or a character
    /// above `U+00FF`.
    fn try_from(row: UserRow) -> Result<Self, Self::Error> {
        Ok(Self {
            sec_usr_id: user_id(&row.user_id, TABLE, "user_id")?,
            sec_usr_fname: alnum::<20>(&row.sec_usr_fname, TABLE, "sec_usr_fname")?,
            sec_usr_lname: alnum::<20>(&row.sec_usr_lname, TABLE, "sec_usr_lname")?,
            password_hash: row.password_hash,
            sec_usr_type: alnum::<1>(&row.sec_usr_type, TABLE, "sec_usr_type")?,
        })
    }
}

/// The text a `user_id` parameter binds, the key right-trimmed, or `None`
/// when the key holds a NUL byte and so names no stored user.
fn key_text(id: &UserId) -> Option<Cow<'_, str>> {
    (!id.as_bytes().contains(&LOW_VALUE)).then(|| id.trimmed())
}

/// The record of a keyed read: no row is the CICS `NOTFND` (`RESP` 13).
fn found(row: Option<UserRow>) -> PortResult<UserSecurity> {
    let row = row.ok_or(PortError::NotFound)?;
    UserSecurity::try_from(row).map_err(PortError::from)
}

/// The failure of a browse whose start key holds a NUL byte: no `user_id`
/// parameter can carry it.
fn unpositionable_start_key() -> PortError {
    PortError::Unavailable("user browse start key holds a NUL byte".to_owned())
}

impl UserStore for PgStore<'_, '_> {
    /// COSGN00C READ-USER-SEC-FILE — P-COSGN00C-008
    /// [app/cbl/COSGN00C.cbl:L209-219]; COUSR03C READ-USER-SEC-FILE —
    /// P-COUSR03C-003 [app/cbl/COUSR03C.cbl:L267-300].
    ///
    /// `READ` by the 8-byte `SEC-USR-ID`: a plain read on the transaction, no
    /// savepoint.
    async fn read_user(&mut self, id: UserId) -> PortResult<UserSecurity> {
        let Some(key) = key_text(&id) else {
            return Err(PortError::NotFound);
        };
        let row = sqlx::query_as!(
            UserRow,
            "SELECT user_id, sec_usr_fname, sec_usr_lname, password_hash, sec_usr_type \
             FROM users WHERE user_id = $1",
            &*key
        )
        .fetch_optional(&mut **self.tx)
        .await
        .map_err(|err| port_error(&err))?;
        found(row)
    }

    /// COUSR02C READ-USER-SEC-FILE — P-COUSR02C-002, P-COUSR02C-003
    /// [app/cbl/COUSR02C.cbl:L320-353]; COUSR03C DELETE-USER-INFO —
    /// P-COUSR03C-004 [app/cbl/COUSR03C.cbl:L174-192].
    ///
    /// `READ … UPDATE` as `SELECT … FOR UPDATE` under its own savepoint. The
    /// row lock outlives the released savepoint and is held until the unit of
    /// work ends, as CICS holds the record until the task's syncpoint.
    async fn read_user_for_update(&mut self, id: UserId) -> PortResult<UserSecurity> {
        let Some(key) = key_text(&id) else {
            return Err(PortError::NotFound);
        };
        let mut sp = savepoint(self.tx).await?;
        let result = sqlx::query_as!(
            UserRow,
            "SELECT user_id, sec_usr_fname, sec_usr_lname, password_hash, sec_usr_type \
             FROM users WHERE user_id = $1 FOR UPDATE",
            &*key
        )
        .fetch_optional(&mut *sp)
        .await;
        found(settle(sp, result).await?)
    }

    /// COUSR00C PROCESS-PAGE-FORWARD, PROCESS-PAGE-BACKWARD — P-COUSR00C-003,
    /// P-COUSR00C-009; STARTBR-USER-SEC-FILE, READNEXT-USER-SEC-FILE,
    /// READPREV-USER-SEC-FILE — P-COUSR00C-006, P-COUSR00C-007,
    /// P-COUSR00C-008 [app/cbl/COUSR00C.cbl:L586-682].
    ///
    /// `STARTBR` positions `GTEQ`, the CICS default (the option is commented
    /// out [app/cbl/COUSR00C.cbl:L592]), then up to `limit` `READNEXT`s
    /// ascending or `READPREV`s descending, the start key included. A `None`
    /// start is `LOW-VALUES` forward and `HIGH-VALUES` backward: no bound. A
    /// start past the last key is the `STARTBR` `NOTFND`, an empty page.
    async fn browse_users(&mut self, browse: Browse<UserId>) -> PortResult<Vec<UserSecurity>> {
        let start = browse
            .start_key
            .as_ref()
            .map(|key| key_text(key).ok_or_else(unpositionable_start_key))
            .transpose()?;
        let start = start.as_deref();
        let limit = limit_i64(browse.limit)?;
        // Bounds and order compare the key padded back to its 8 bytes, the
        // order of `UserId`'s `Ord`. The right-trimmed column alone would
        // sort `AB` before `AB` followed by a byte below the space, where the
        // COBOL compares its padding space with that byte and sorts it after.
        let rows = match browse.direction {
            Direction::Forward => {
                sqlx::query_as!(
                    UserRow,
                    r#"SELECT user_id, sec_usr_fname, sec_usr_lname, password_hash, sec_usr_type
                       FROM users
                       WHERE $1::text IS NULL OR rpad(user_id, 8) COLLATE "C" >= rpad($1, 8)
                       ORDER BY rpad(user_id, 8) COLLATE "C" ASC
                       LIMIT $2"#,
                    start,
                    limit
                )
                .fetch_all(&mut **self.tx)
                .await
            }
            Direction::Backward => {
                sqlx::query_as!(
                    UserRow,
                    r#"SELECT user_id, sec_usr_fname, sec_usr_lname, password_hash, sec_usr_type
                       FROM users
                       WHERE $1::text IS NULL OR rpad(user_id, 8) COLLATE "C" <= rpad($1, 8)
                       ORDER BY rpad(user_id, 8) COLLATE "C" DESC
                       LIMIT $2"#,
                    start,
                    limit
                )
                .fetch_all(&mut **self.tx)
                .await
            }
        }
        .map_err(|err| port_error(&err))?;
        rows.into_iter()
            .map(|row| UserSecurity::try_from(row).map_err(PortError::from))
            .collect()
    }

    /// COUSR01C WRITE-USER-SEC-FILE — P-COUSR01C-004, P-COUSR01C-005
    /// [app/cbl/COUSR01C.cbl:L238-274].
    ///
    /// `WRITE` as `INSERT` under its own savepoint. An existing ID is the
    /// `DUPKEY`/`DUPREC` of the COBOL: SQLSTATE 23505, [`PortError::Duplicate`].
    async fn insert_user(&mut self, user: UserSecurity) -> PortResult<()> {
        let key = user.sec_usr_id.trimmed();
        let fname = user.sec_usr_fname.trimmed();
        let lname = user.sec_usr_lname.trimmed();
        let user_type = user.sec_usr_type.trimmed();
        let mut sp = savepoint(self.tx).await?;
        let result = sqlx::query!(
            "INSERT INTO users (user_id, sec_usr_fname, sec_usr_lname, password_hash, \
             sec_usr_type) VALUES ($1, $2, $3, $4, $5)",
            &*key,
            &*fname,
            &*lname,
            user.password_hash.as_str(),
            &*user_type
        )
        .execute(&mut *sp)
        .await;
        settle(sp, result).await?;
        Ok(())
    }

    /// COUSR02C UPDATE-USER-SEC-FILE — P-COUSR02C-006
    /// [app/cbl/COUSR02C.cbl:L358-390].
    ///
    /// `REWRITE` of every field but the key, keyed by `sec_usr_id`, under its
    /// own savepoint. No row with the key is the `NOTFND` of the COBOL.
    async fn update_user(&mut self, user: UserSecurity) -> PortResult<()> {
        let Some(key) = key_text(&user.sec_usr_id) else {
            return Err(PortError::NotFound);
        };
        let fname = user.sec_usr_fname.trimmed();
        let lname = user.sec_usr_lname.trimmed();
        let user_type = user.sec_usr_type.trimmed();
        let mut sp = savepoint(self.tx).await?;
        let result = sqlx::query!(
            "UPDATE users SET sec_usr_fname = $2, sec_usr_lname = $3, password_hash = $4, \
             sec_usr_type = $5 WHERE user_id = $1",
            &*key,
            &*fname,
            &*lname,
            user.password_hash.as_str(),
            &*user_type
        )
        .execute(&mut *sp)
        .await;
        let done = settle(sp, result).await?;
        expect_affected(done.rows_affected())
    }

    /// COUSR03C DELETE-USER-SEC-FILE — P-COUSR03C-005
    /// [app/cbl/COUSR03C.cbl:L305-336].
    ///
    /// `DELETE` by key under its own savepoint. No row with the key is the
    /// `NOTFND` of the COBOL.
    async fn delete_user(&mut self, id: UserId) -> PortResult<()> {
        let Some(key) = key_text(&id) else {
            return Err(PortError::NotFound);
        };
        let mut sp = savepoint(self.tx).await?;
        let result = sqlx::query!("DELETE FROM users WHERE user_id = $1", &*key)
            .execute(&mut *sp)
            .await;
        let done = settle(sp, result).await?;
        expect_affected(done.rows_affected())
    }
}

#[cfg(test)]
mod tests {
    use carddemo_domain::entities::UserSecurity;
    use carddemo_domain::numeric::{Alnum, UserId};
    use carddemo_domain::ports::PortError;

    use super::{PersistenceError, UserRow, found, key_text};

    /// A well-formed argon2id PHC string with a made-up salt and output.
    const HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$c2FsdHNhbHRzYWx0$aGFzaGhhc2hoYXNoaGFzaGhhc2hoYXNoaGFzaGhhc2g";

    /// The stored form of the seeded `ADMIN001` row [app/jcl/DUSRSECJ.jcl:L35].
    fn admin001_row() -> UserRow {
        UserRow {
            user_id: "ADMIN001".to_owned(),
            sec_usr_fname: "MARGARET".to_owned(),
            sec_usr_lname: "GOLD".to_owned(),
            password_hash: HASH.to_owned(),
            sec_usr_type: "A".to_owned(),
        }
    }

    #[test]
    fn a_stored_row_is_padded_back_and_keeps_its_hash() -> Result<(), PersistenceError> {
        let user = UserSecurity::try_from(admin001_row())?;
        assert_eq!(user.sec_usr_id.as_bytes(), b"ADMIN001");
        assert_eq!(user.sec_usr_fname.as_bytes(), b"MARGARET            ");
        assert_eq!(user.sec_usr_lname.as_bytes(), b"GOLD                ");
        assert_eq!(user.password_hash, HASH);
        assert_eq!(user.sec_usr_type.as_bytes(), b"A");

        let blank_type = UserSecurity::try_from(UserRow {
            user_id: "user01".to_owned(),
            sec_usr_type: String::new(),
            ..admin001_row()
        })?;
        assert_eq!(blank_type.sec_usr_id.as_bytes(), b"user01  ");
        assert_eq!(blank_type.sec_usr_type, Alnum::spaces());
        Ok(())
    }

    #[test]
    fn a_value_that_does_not_fit_its_field_is_corrupt_naming_the_column() {
        let cases = [
            (
                UserRow {
                    user_id: "ADMIN0012".to_owned(),
                    ..admin001_row()
                },
                "user_id",
                "expected at most 8 characters, got 9",
            ),
            (
                UserRow {
                    sec_usr_fname: "F".repeat(21),
                    ..admin001_row()
                },
                "sec_usr_fname",
                "expected at most 20 characters, got 21",
            ),
            (
                UserRow {
                    sec_usr_lname: "G\u{100}".to_owned(),
                    ..admin001_row()
                },
                "sec_usr_lname",
                "value has a character outside ISO 8859-1",
            ),
            (
                UserRow {
                    sec_usr_type: "AU".to_owned(),
                    ..admin001_row()
                },
                "sec_usr_type",
                "expected at most 1 characters, got 2",
            ),
        ];
        for (row, expected_column, expected_reason) in cases {
            match UserSecurity::try_from(row) {
                Err(PersistenceError::Corrupt {
                    table,
                    column,
                    reason,
                }) => {
                    assert_eq!(table, "users");
                    assert_eq!(column, expected_column);
                    assert_eq!(reason, expected_reason, "{expected_column}");
                    assert!(!reason.contains(HASH), "{reason}");
                }
                other => panic!("{expected_column}: expected Corrupt, got {other:?}"),
            }
        }
    }

    #[test]
    fn keys_bind_right_trimmed_and_a_nul_key_binds_nothing() {
        let cases = [
            (UserId::move_from("ADMIN001"), Some("ADMIN001")),
            (UserId::move_from("user01"), Some("user01")),
            (UserId::move_from(" AB"), Some(" AB")),
            (UserId::from_alnum(Alnum::spaces()), Some("")),
            (
                UserId::from_alnum(Alnum::from_array(*b"ADM\xE9N1  ")),
                Some("ADM\u{e9}N1"),
            ),
            (UserId::from_alnum(Alnum::from_array(*b"AB\0     ")), None),
            (UserId::from_alnum(Alnum::from_array([0; 8])), None),
        ];
        for (id, expected) in cases {
            assert_eq!(key_text(&id).as_deref(), expected, "{id:?}");
        }
    }

    #[test]
    fn a_keyed_read_of_no_row_is_not_found() {
        assert!(matches!(found(None), Err(PortError::NotFound)));
        let user = found(Some(admin001_row())).map(|user| user.sec_usr_id);
        assert_eq!(user, Ok(UserId::move_from("ADMIN001")));
    }
}
