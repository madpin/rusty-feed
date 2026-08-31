-- One active embedding space per installation. A model change must go through
-- the explicit CLI reindex flow; normal startup never silently mixes vectors.
CREATE TABLE instance_embedding_space (
    singleton  BOOLEAN PRIMARY KEY DEFAULT true CHECK (singleton),
    model      TEXT NOT NULL,
    dimensions INT NOT NULL CHECK (dimensions > 0),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

