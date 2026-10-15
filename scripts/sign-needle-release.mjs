// Explicit release step; development builds never acquire a release identity.
import { join } from "node:path";
import { root, json } from "./tooling.mjs";
import { signNeedle } from "./sign-update.mjs";
import { verifyArtifact } from "./ci/artifact.mjs";

verifyArtifact("resources/agents");
signNeedle(join(root, "resources/agents"), json(join(root, "resources/app.json")).version);
