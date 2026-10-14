CREATE TYPE account.koru_account_home_region AS ENUM ('US');

ALTER TABLE account.koru_identities
    ADD COLUMN home_region account.koru_account_home_region NOT NULL DEFAULT 'US';
