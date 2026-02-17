import { Database } from "bun:sqlite";

const CHARSET = "abcdefghijklmnopqrstuvwxyz0123456789";
const ID_LEN = 8;

function generateId(): string {
  let id = "";
  for (let i = 0; i < ID_LEN; i++) {
    id += CHARSET[Math.floor(Math.random() * CHARSET.length)];
  }
  return id;
}

const dbPath = process.argv[2] || "data/monoize.db";
const db = new Database(dbPath);

db.run("PRAGMA foreign_keys = OFF");

const providers = db
  .query<{ id: string }, []>("SELECT id FROM monoize_providers")
  .all();

if (providers.length === 0) {
  console.log("No providers to migrate.");
  process.exit(0);
}

const existingIds = new Set(providers.map((p) => p.id));

const txn = db.transaction(() => {
  for (const { id: oldId } of providers) {
    let newId: string;
    do {
      newId = generateId();
    } while (existingIds.has(newId));
    existingIds.add(newId);

    db.run("UPDATE monoize_provider_models SET provider_id = ? WHERE provider_id = ?", [newId, oldId]);
    db.run("UPDATE monoize_channels SET provider_id = ? WHERE provider_id = ?", [newId, oldId]);
    db.run("UPDATE monoize_providers SET id = ? WHERE id = ?", [newId, oldId]);

    console.log(`${oldId} -> ${newId}`);
  }
});

txn();

db.run("PRAGMA foreign_keys = ON");
console.log(`Migrated ${providers.length} provider(s).`);
