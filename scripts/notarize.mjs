import { capture } from "./tooling.mjs";

export function notarizationAuth(env = process.env) {
  if (env.NEWPORT_NOTARY_PROFILE)
    return ["--keychain-profile", env.NEWPORT_NOTARY_PROFILE];
  for (const name of ["APPLE_ID", "APPLE_PASSWORD", "APPLE_TEAM_ID"])
    if (!env[name])
      throw new Error(
        "Set NEWPORT_NOTARY_PROFILE or APPLE_ID, APPLE_PASSWORD and APPLE_TEAM_ID",
      );
  return [
    "--apple-id",
    env.APPLE_ID,
    "--password",
    env.APPLE_PASSWORD,
    "--team-id",
    env.APPLE_TEAM_ID,
  ];
}

export function submitNotarization(archive, auth) {
  let output;
  try {
    output = capture(
      "xcrun",
      [
        "notarytool",
        "submit",
        archive,
        ...auth,
        "--wait",
        "--output-format",
        "json",
      ],
      { stdio: ["ignore", "pipe", "pipe"] },
    );
  } catch {
    // execFileSync errors include the full command, including its password.
    throw new Error("Native notarization submission failed");
  }
  const result = JSON.parse(output);
  if (result.status !== "Accepted")
    throw new Error(
      `Native notarization was not accepted (${result.status ?? "unknown"})`,
    );
}
