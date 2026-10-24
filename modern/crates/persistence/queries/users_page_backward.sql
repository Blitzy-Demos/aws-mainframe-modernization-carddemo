-- COUSR00C PROCESS-PAGE-BACKWARD (app/cbl/COUSR00C.cbl:336-379): STARTBR (default GTEQ, 586-614)
-- + READPREV (653-682) over USRSEC by SEC-USR-ID (KEYS(8,0), app/jcl/DUSRSECJ.jcl:65), in
-- descending key byte order, the reverse of the "C" collation migration 0003 gives usr_id, which
-- orders the stored ASCII bytes, whereas the KSDS ordered EBCDIC bytes. The two orders agree for
-- the fixture user IDs, ADMIN001 to ADMIN005 and USER0001 to USER0005, which first differ letter
-- against letter or digit against digit. They need not agree for arbitrary IDs, which can first
-- differ digit against letter or lower case against upper case (escalation candidate
-- X-COUSR00C-2 in modern/audit/online/COUSR00C.md). The first READPREV after STARTBR returns the
-- start record, which F7 discards, so the start key is inclusive. Audit rows COUSR00C-022 to
-- COUSR00C-024. Only the four columns POPULATE-USER-DATA shows are read, and no credential
-- column. Endpoint 16 GET /api/v1/users [A] (F7). Rule: Bounded Pagination (users_pkey, scanned
-- backward).
-- Parameters: 1 start user ID, inclusive (the first user ID shown, or empty for LOW-VALUES),
-- 2 row limit.
SELECT
    usr_id,
    usr_fname,
    usr_lname,
    usr_type
FROM users
WHERE usr_id <= $1::bpchar
ORDER BY usr_id DESC
LIMIT $2::bigint
