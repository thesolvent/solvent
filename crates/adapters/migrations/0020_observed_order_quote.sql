-- What sourcing the order would have cost us, once the decision loop priced it. Without it the
-- feed answers "we refused this" but not "by how much", which is the difference between a pair we
-- are close on and one we should not be quoting at all.
ALTER TABLE observed_order ADD COLUMN indicative_in TEXT;
