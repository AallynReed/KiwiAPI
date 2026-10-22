// Twin sandbox seed for file drops: links minted by Python (argon2 PIN 1357)
// and an upload row whose file is not on disk.
db.file_drops.insertOne({_id: ObjectId("66f0000000000000000000a1"), slug: "parity-seeded-link", label: "Seeded",
  pin_hash: "$argon2id$v=19$m=65536,t=3,p=4$RM9VcmPk3cZQPhho9RyOpg$XEvMkyfOf9XX7c8alP8mvn8G6ZS4jgSsRqlAUnl1G2s",
  max_uploads: NumberInt(3), upload_count: NumberInt(0), max_file_bytes: NumberInt(1048576),
  expires_at: ISODate("2099-01-01T00:00:00Z"), revoked: false, created_at: ISODate("2026-01-01T00:00:00Z")});
db.file_drops.insertOne({_id: ObjectId("66f0000000000000000000a2"), slug: "parity-expired-link", label: "Expired",
  pin_hash: "$argon2id$v=19$m=65536,t=3,p=4$RM9VcmPk3cZQPhho9RyOpg$XEvMkyfOf9XX7c8alP8mvn8G6ZS4jgSsRqlAUnl1G2s",
  max_uploads: NumberInt(1), upload_count: NumberInt(0), max_file_bytes: NumberInt(1048576),
  expires_at: ISODate("2026-01-02T00:00:00Z"), revoked: false, created_at: ISODate("2026-01-01T00:00:00Z")});
db.drop_uploads.insertOne({_id: ObjectId("66f1000000000000000000a1"), drop_id: ObjectId("66f0000000000000000000a1"),
  filename: "gone.txt", stored_name: "0000000000000000-gone.txt", size: NumberInt(5), content_type: "text/plain",
  sha256: "735c26a70304d4b23dc41bc5f3305cb4c5177a364f3c8ba40f68f1aec9b2fea6", note: null,
  uploaded_at: ISODate("2026-01-01T01:00:00Z")});
