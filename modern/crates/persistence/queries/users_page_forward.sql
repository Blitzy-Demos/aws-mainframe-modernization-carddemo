-- COUSR00C PROCESS-PAGE-FORWARD (app/cbl/COUSR00C.cbl:282-331): STARTBR (default GTEQ, 586-614)
-- + READNEXT (619-648) over USRSEC by SEC-USR-ID (KEYS(8,0), app/jcl/DUSRSECJ.jcl:65), in
-- ascending key byte order. The "C" collation migration 0003 gives usr_id orders the stored ASCII
-- bytes, whereas the KSDS ordered EBCDIC bytes. The two orders agree for the fixture user IDs,
-- ADMIN001 to ADMIN005 and USER0001 to USER0005, which first differ letter against letter or
-- digit against digit. They need not agree for arbitrary IDs, which can first differ digit
-- against letter or lower case against upper case (escalation candidate X-COUSR00C-2 in
-- modern/audit/online/COUSR00C.md). Audit rows COUSR00C-013 to COUSR00C-018. Only the four
-- columns POPULATE-USER-DATA shows are read, and no credential column. Endpoint 16
-- GET /api/v1/users [A]. Rule: Bounded Pagination (users_pkey).
-- Parameters: 1 start user ID, inclusive (empty for the LOW-VALUES first page), 2 row limit.
SELECT
    usr_id,
    usr_fname,
    usr_lname,
    usr_type
FROM users
WHERE usr_id >= $1::bpchar
ORDER BY usr_id ASC
LIMIT $2::bigint
