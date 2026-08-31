-- Public-launch AI cost enforcement (FND-187).
--
-- Distinguish instance-funded spend (admin/environment key) from bring-your-own
-- key spend so the per-instance and per-user USD budgets can exempt users who
-- pay with their own enabled personal key (docs/PUBLIC_LAUNCH_PLAN.md §4.1, §8.1).
-- Embeddings are also metered from now on (kind='embedding'); shared article
-- embeddings carry user_id = NULL, interactive query embeddings carry the
-- requesting user's id so per-user daily caps can be enforced.
ALTER TABLE cost_ledger
    ADD COLUMN billed_to_user BOOLEAN NOT NULL DEFAULT false;

-- Fast "today's usage for this user" lookups used by the budget checks.
CREATE INDEX cost_ledger_user_kind_time_idx
    ON cost_ledger (user_id, kind, occurred_at);
