// Runs once, on a FRESH data dir, after the root user is created. Creates a
// least-privilege application user (readWrite on the app DB only). The app
// connects as this user, never as root.
const appDb = db.getSiblingDB(process.env.MONGO_INITDB_DATABASE);
appDb.createUser({
  user: process.env.MONGO_APP_USER,
  pwd: process.env.MONGO_APP_PASSWORD,
  roles: [{ role: "readWrite", db: process.env.MONGO_INITDB_DATABASE }],
});
