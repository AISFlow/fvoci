-- Existing numeric estimates have no known time unit. Preserve every value;
-- only an explicit minute command establishes the new nullable unit.
ALTER TABLE fvoci.tasks ADD COLUMN estimate_unit text;
ALTER TABLE fvoci.tasks ADD CONSTRAINT task_estimate_explicit_minutes CHECK (
    estimate_unit IS NULL OR (
        estimate_unit = 'minutes' AND estimate IS NOT NULL
        AND estimate >= 0 AND estimate = trunc(estimate)
        AND estimate <= 2147483647
    )
);
