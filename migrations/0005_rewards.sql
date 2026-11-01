-- Rewards schema: the configurable (type, category) -> rate reward rules, the reward balance
-- of each account and the accrual history behind it (o8). It holds no rows: `batch seed`
-- inserts the seed rules (01/0001 at 1.0000 and 04/0003 at 3.0000, both active from
-- 2022-01-01T00:00:00Z), and `batch posttran` over dailytran.txt accrues the seed balances
-- through the RewardEngine (crates/domain/src/rewards/engine.rs).
--
-- Source: request Rewards system; AAP 0.3.5. Seed rules: AAP 0.3.6. Numeric types:
--   AAP 0.5.3.1. No COBOL program computes rewards: the README lists DB2 Rewards as a
--   roadmap item [README.md:L357-369]. No parity audit row gates this file, and no column
--   follows a copybook field. Two widths come from the COBOL: the amount bounds take the
--   S9(09)V99 of TRAN-AMT [app/cpy/CVTRA05Y.cpy:L10] and DALYTRAN-AMT
--   [app/cpy/CVTRA06Y.cpy:L10], the amounts the engine compares them with, and
--   reward_balances.account_id is the ACCT-ID 9(11) key of accounts
--   [app/cpy/CVACT01Y.cpy:L5].
--
-- Conventions (shared with 0001 to 0003 where a column has a counterpart there):
--   The codes keep the types of the transaction columns they are matched against, so a rule
--   match is a plain equality: the type code is VARCHAR(2) COLLATE "C" like
--   transactions.tran_type_cd, and the category code is INTEGER 0 to 9999 like
--   transactions.tran_cat_cd (the fixture category 0001 is stored as 1).
--   Amounts are NUMERIC(11,2): the 9 integer digits and 2 decimals of S9(09)V99.
--   reward_rate is NUMERIC(9,4): at most 5 integer and 4 fraction digits, the RewardRate
--   range. PostgreSQL rounds a value with more fraction digits to 4; RewardRate rejects a
--   non-zero fifth fraction digit (TooManyDecimals) before any value reaches SQL.
--   Points are BIGINT, the 0 to i64::MAX range of RewardPoints. The API carries them as
--   decimal digit strings, so a value above 2^53 - 1, such as 9007199254740993, reaches the
--   browser exactly.
--   Instants are TIMESTAMPTZ. No column defaults to the current time: writers supply the
--   processing clock (crates/domain/src/clock.rs, CARDDEMO_CLOCK_OVERRIDE when set), so
--   accrual and soft-delete instants follow the pinned clock in tests.
--   rule_id and accrual_id are GENERATED ALWAYS identities; no writer supplies them.
--   Nothing here blocks TRUNCATE reward_accruals, reward_balances, reward_rules
--   RESTART IDENTITY, the seed refresh that restarts rule IDs at 1.
--   PostgreSQL refuses to TRUNCATE a referenced table unless every table referencing it is
--   truncated in the same statement, whether or not rows exist: a TRUNCATE of accounts
--   must also name reward_balances, and one of transactions must name reward_accruals. A
--   DELETE needs only the referencing rows removed first.
--
-- reward_rules: one rule per (type, category, rate, amount range, active window)
--   column                  | meaning                                             | SQL type                        | Rust type
--   rule_id                 | rule identity; the lowest wins a tie of equal rates | BIGINT NOT NULL, identity, PK   | RuleId (i64)
--   transaction_type_cd     | transaction type code the rule matches              | VARCHAR(2) COLLATE "C" NOT NULL | TranTypeCode
--   transaction_category_cd | transaction category code the rule matches          | INTEGER NOT NULL, 0 to 9999     | TranCategoryCode
--   reward_rate             | points per currency unit; the highest match wins    | NUMERIC(9,4) NOT NULL, >= 0     | RewardRate
--   min_transaction_amount  | inclusive lower amount bound; NULL is unbounded     | NUMERIC(11,2) NULL              | Option<Money<9>>
--   max_transaction_amount  | inclusive upper amount bound; NULL is unbounded     | NUMERIC(11,2) NULL              | Option<Money<9>>
--   active_from             | first instant the rule is active (inclusive)        | TIMESTAMPTZ NOT NULL            | DateTime<Utc>
--   active_to               | end of the window (exclusive); NULL is open-ended   | TIMESTAMPTZ NULL                | Option<DateTime<Utc>>
--   A rule is active at instant t when active_from <= t and (active_to IS NULL or
--   t < active_to). It matches an amount a when it is active at the processing clock, both
--   codes equal the transaction's, and a lies within its bounds.
--
-- reward_balances: the reward balance of one account
--   column                | meaning                                               | SQL type                         | Rust type
--   account_id            | account the balance belongs to                        | BIGINT NOT NULL, PK, FK accounts | AccountId
--   total_points_earned   | sum of the account's accruals                         | BIGINT NOT NULL DEFAULT 0, >= 0  | RewardPoints (i64)
--   total_points_redeemed | points spent; redemption is out of scope, so always 0 | BIGINT NOT NULL DEFAULT 0, >= 0  | RewardPoints (i64)
--   available_points      | total_points_earned - total_points_redeemed           | BIGINT NOT NULL DEFAULT 0        | RewardPoints (i64)
--
-- reward_accruals: the points one qualifying transaction earned
--   column         | meaning                                      | SQL type                                 | Rust type
--   accrual_id     | accrual identity; reward history page key    | BIGINT NOT NULL, identity, PK            | AccrualId (i64)
--   account_id     | account credited                             | BIGINT NOT NULL                          | AccountId
--   transaction_id | transaction row that earned the points       | BIGINT NOT NULL, unique, FK transactions | TransactionRowId (i64)
--   rule_id        | winning rule                                 | BIGINT NOT NULL, FK reward_rules         | RuleId (i64)
--   points_earned  | ceil(amount x reward_rate)                   | BIGINT NOT NULL, >= 0                    | RewardPoints (i64)
--   created_at     | processing clock instant of the accrual      | TIMESTAMPTZ NOT NULL                     | DateTime<Utc>
--
-- Writers. crates/persistence/src/rewards.rs (PgRewardStore) reads the candidate rules,
-- inserts the accrual and upserts the balance inside the caller's transaction: CT02 commits
-- them with the transaction insert (POST /transactions), POSTTRAN with the CBTRN02C updates
-- of each posted record. The /admin/reward-rules handlers create, update and soft-delete
-- rules.

-- The window rule active_to >= active_from is not a table constraint. The API enforces it
-- on POST and PUT /admin/reward-rules (400, 'active_to must not precede active_from'), but
-- DELETE soft-deletes by setting active_to to the processing clock whenever it is NULL or
-- later. For a rule whose active_from is still ahead, that instant precedes active_from,
-- and the delete must still succeed (200); the rule is then never active, as no instant t
-- satisfies active_from <= t < active_to.
-- Rules are keyed by code values only, with no foreign key to transaction_types,
-- transaction_categories, trantype_file or trancatg_file: the codes must survive a seed
-- refresh that reloads the processing copies, and a CTLI or MNTTRDB2 delete of a master
-- type must not fail on a reward rule the COBOL delete paths know nothing of.
CREATE TABLE reward_rules (
    rule_id                 BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    transaction_type_cd     VARCHAR(2) COLLATE "C" NOT NULL,
    transaction_category_cd INTEGER                NOT NULL
        CHECK (transaction_category_cd >= 0 AND transaction_category_cd < 10000),
    reward_rate             NUMERIC(9,4)           NOT NULL
        CHECK (reward_rate >= 0),
    min_transaction_amount  NUMERIC(11,2)          NULL,
    max_transaction_amount  NUMERIC(11,2)          NULL,
    active_from             TIMESTAMPTZ            NOT NULL,
    active_to               TIMESTAMPTZ            NULL,
    CONSTRAINT reward_rules_amount_range_check CHECK (
        min_transaction_amount IS NULL
        OR max_transaction_amount IS NULL
        OR min_transaction_amount <= max_transaction_amount
    )
);

-- Serves the candidate-rule read of PgRewardStore: equality on both codes, then the active
-- window at the processing clock.
CREATE INDEX reward_rules_match_idx
    ON reward_rules (transaction_type_cd, transaction_category_cd, active_from, active_to);

-- One row per account that has accrued; an existing account without a row has a zero
-- balance. The engine computes available_points by the same subtraction before the upsert,
-- and the check holds the invariant for every other writer too, such as the e2e fixture's
-- psql update.
CREATE TABLE reward_balances (
    account_id            BIGINT NOT NULL PRIMARY KEY,
    total_points_earned   BIGINT NOT NULL DEFAULT 0
        CHECK (total_points_earned >= 0),
    total_points_redeemed BIGINT NOT NULL DEFAULT 0
        CHECK (total_points_redeemed >= 0),
    available_points      BIGINT NOT NULL DEFAULT 0,
    CONSTRAINT reward_balances_account_id_fkey
        FOREIGN KEY (account_id) REFERENCES accounts (account_id),
    CONSTRAINT reward_balances_available_points_check
        CHECK (available_points = total_points_earned - total_points_redeemed)
);

-- points_earned is 0 when the winning rule's rate is 0: the transaction still qualified,
-- so its accrual is recorded.
-- ON DELETE RESTRICT on transaction_id is the E-09 constraint: `batch tranbkp` deletes only
-- the non-live transactions no accrual references, so every accrual keeps its transaction
-- and the reward history outlives the emptied live master
-- (migrations/0003_transactions.sql). The unique key allows one accrual per qualifying
-- transaction, and its index serves the RESTRICT check of that delete.
-- account_id carries no foreign key: the request's model names only the three below. The
-- writer takes it from the account the transaction posted to, whose reward_balances row,
-- upserted in the same database transaction, references accounts.
CREATE TABLE reward_accruals (
    accrual_id     BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    account_id     BIGINT      NOT NULL,
    transaction_id BIGINT      NOT NULL,
    rule_id        BIGINT      NOT NULL,
    points_earned  BIGINT      NOT NULL
        CHECK (points_earned >= 0),
    created_at     TIMESTAMPTZ NOT NULL,
    CONSTRAINT reward_accruals_transaction_id_fkey
        FOREIGN KEY (transaction_id) REFERENCES transactions (transaction_id)
        ON DELETE RESTRICT,
    CONSTRAINT reward_accruals_rule_id_fkey
        FOREIGN KEY (rule_id) REFERENCES reward_rules (rule_id),
    CONSTRAINT reward_accruals_transaction_id_key UNIQUE (transaction_id)
);

-- Serves the reward history of one account (GET /rewards/history/{account_id}), which
-- filters on account_id; created_at orders an account's accruals in time.
CREATE INDEX reward_accruals_account_created_idx
    ON reward_accruals (account_id, created_at);
