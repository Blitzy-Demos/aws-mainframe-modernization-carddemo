-- User security table, in place of the USRSEC KSDS that DUSRSECJ defines and loads. A row is
-- the 80-byte SEC-USER-DATA record [app/cpy/CSUSR01Y.cpy:17-23], columns in copybook order. The
-- primary key is the cluster's KEYS(8,0) [app/jcl/DUSRSECJ.jcl:65], RECORDSIZE(80,80)
-- [app/jcl/DUSRSECJ.jcl:66]. Field-to-column rows: CSUSR01Y-001 to -006 in
-- modern/audit/copybooks.md.
--
-- E25 (modern/audit/ESCALATIONS.md): SEC-USR-PWD X(08) [app/cpy/CSUSR01Y.cpy:21] is plaintext
-- and has no column. password_hash holds the argon2id PHC string that domain::password computes
-- over exactly the 8 stored password bytes, so codec B is not byte-identical for that field.
-- USRSEC is outside the 626 fixture records, and codec A still round-trips each record. Nothing
-- but the hash is stored (Cardholder Data Redaction).
--
-- Every character column is COLLATE "C", as in migration 0001: the KSDS orders keys by their
-- bytes, and the server's default locale must not change the order in which the COUSR00C browse
-- (endpoint 16) pages on users_pkey. That order is ASCII byte order; the USRSEC KSDS ordered
-- EBCDIC bytes. The two agree for the fixture user IDs, ADMIN001 to ADMIN005 and USER0001 to
-- USER0005: upper-case letters then digits, so where two of them first differ, a letter meets a
-- letter or a digit meets a digit. They need not agree for arbitrary IDs, where a digit can meet a
-- letter or lower case meet upper case at that byte (escalation candidate X-COUSR00C-2 in
-- modern/audit/online/COUSR00C.md).
--
-- dusrsecj replaces the table's contents inside one transaction, as the IDCAMS DELETE, DEFINE
-- and REPRO of DUSRSECJ do. No foreign key references the table. Only runtime::migrate applies
-- this file (Single Schema Executor); a later schema change is a new numbered file, never an
-- edit of this one.

CREATE TABLE users (
    usr_id        CHAR(8)  COLLATE "C" NOT NULL,
    usr_fname     CHAR(20) COLLATE "C" NOT NULL,
    usr_lname     CHAR(20) COLLATE "C" NOT NULL,
    password_hash TEXT     COLLATE "C" NOT NULL,
    -- No value-list CHECK: the column holds whatever the COUSR01C and COUSR02C edits accept;
    -- the admin gate tests for A.
    usr_type      CHAR(1)  COLLATE "C" NOT NULL,
    -- SEC-USR-FILLER X(23), kept byte for byte. The in-stream lines end before it, so it holds
    -- 23 spaces once dusrsecj pads each line to 80 bytes (X-COPYBOOKS-1 in
    -- modern/audit/copybooks.md).
    filler        CHAR(23) COLLATE "C" NOT NULL,
    CONSTRAINT users_pkey PRIMARY KEY (usr_id)
);
