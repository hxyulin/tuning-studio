import { cpSync, rmSync } from "node:fs";
rmSync("src-tauri/frontend", { recursive: true, force: true });
cpSync("dist", "src-tauri/frontend", { recursive: true });

rmSync("crates/studio-server/frontend", { recursive: true, force: true });
cpSync("dist", "crates/studio-server/frontend", { recursive: true });
