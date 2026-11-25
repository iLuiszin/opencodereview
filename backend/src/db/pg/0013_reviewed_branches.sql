ALTER TABLE subscriptions
  ADD COLUMN review_reviewed_branches TEXT COLLATE "C" NOT NULL DEFAULT '[]';
