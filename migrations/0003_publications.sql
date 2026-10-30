CREATE TABLE publications (
    destination TEXT NOT NULL,
    channel TEXT NOT NULL CHECK(channel IN ('daily', 'test')),
    revision TEXT NOT NULL,
    image_id TEXT NOT NULL,
    status TEXT NOT NULL,
    error TEXT,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY(destination, channel, revision, image_id)
);

CREATE TABLE uploaded_objects (
    destination TEXT NOT NULL,
    object_key TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    size_bytes INTEGER NOT NULL,
    verified_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY(destination, object_key)
);

CREATE TABLE publication_heads (
    destination TEXT NOT NULL,
    channel TEXT NOT NULL,
    revision TEXT NOT NULL,
    image_id TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY(destination, channel)
);
