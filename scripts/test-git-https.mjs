/** A real authenticated HTTPS Git endpoint; no external server or user credentials. */
import { createServer } from "node:https";
import { spawn } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { root, run, capture } from "./tooling.mjs";
const temp = mkdtempSync(join(tmpdir(), "newport-https-"));
let server, test;
const backends = new Set();
try {
  const cert = join(temp, "cert.pem"),
    key = join(temp, "key.pem");
  run(
    "openssl",
    [
      "req",
      "-x509",
      "-newkey",
      "rsa:2048",
      "-nodes",
      "-keyout",
      key,
      "-out",
      cert,
      "-days",
      "1",
      "-subj",
      "/CN=localhost",
      "-addext",
      "subjectAltName=IP:127.0.0.1",
    ],
    { stdio: "ignore" },
  );
  const env = {
    ...process.env,
    GIT_CONFIG_NOSYSTEM: "1",
    GIT_CONFIG_GLOBAL: "/dev/null",
    GIT_TERMINAL_PROMPT: "0",
  };
  run(
    "git",
    ["init", "--bare", "--initial-branch=main", join(temp, "repo.git")],
    { env },
  );
  run(
    "git",
    ["-C", join(temp, "repo.git"), "config", "http.receivepack", "true"],
    { env },
  );
  const backend = join(capture("git", ["--exec-path"]), "git-http-backend");
  const authorization = `Basic ${Buffer.from("fixture:fixture-token").toString("base64")}`;
  let authenticated = 0,
    denied = 0;
  server = createServer(
    { key: readFileSync(key), cert: readFileSync(cert) },
    async (req, res) => {
      if (req.headers.authorization !== authorization) {
        denied++;
        res.writeHead(401, {
          "WWW-Authenticate": 'Basic realm="fixture"',
          "Content-Length": "0",
        });
        res.end();
        return;
      }
      authenticated++;
      try {
        const url = new URL(req.url, "https://127.0.0.1");
        if (!url.pathname.startsWith("/repo.git/")) {
          res.writeHead(404);
          res.end();
          return;
        }
        let length = 0;
        const chunks = [];
        for await (const chunk of req) {
          length += chunk.length;
          if (length > 8 * 1024 * 1024)
            throw Error("Oversized fixture request");
          chunks.push(chunk);
        }
        const child = spawn(backend, [], {
          env: {
            ...env,
            GIT_PROJECT_ROOT: temp,
            GIT_HTTP_EXPORT_ALL: "1",
            PATH_INFO: url.pathname,
            QUERY_STRING: url.search.slice(1),
            REQUEST_METHOD: req.method,
            REMOTE_USER: "fixture",
            CONTENT_TYPE: req.headers["content-type"] || "",
            CONTENT_LENGTH: String(length),
          },
          stdio: ["pipe", "pipe", "pipe"],
        });
        backends.add(child);
        const timer = setTimeout(() => child.kill("SIGKILL"), 10000);
        const output = [];
        let outputSize = 0;
        child.stdout.on("data", (b) => {
          outputSize += b.length;
          if (outputSize > 16 * 1024 * 1024) child.kill("SIGKILL");
          else output.push(b);
        });
        child.stderr.resume();
        child.stdin.on("error", () => {});
        child.stdin.end(Buffer.concat(chunks));
        try {
          await new Promise((resolve, reject) => {
            child.once("error", reject);
            child.once("exit", (code) =>
              code === 0 ? resolve() : reject(Error("Git HTTP fixture failed")),
            );
          });
        } finally {
          clearTimeout(timer);
          backends.delete(child);
        }
        const bytes = Buffer.concat(output),
          split = bytes.indexOf("\r\n\r\n");
        if (split < 0) throw Error("Invalid CGI response");
        const headers = bytes.subarray(0, split).toString().split("\r\n");
        let status = 200;
        for (const header of headers) {
          const colon = header.indexOf(":");
          const name = header.slice(0, colon),
            value = header.slice(colon + 1).trim();
          if (name.toLowerCase() === "status")
            status = Number(value.split(" ")[0]);
          else res.setHeader(name, value);
        }
        res.writeHead(status);
        res.end(bytes.subarray(split + 4));
      } catch (error) {
        console.error(error.message);
        res.writeHead(500);
        res.end();
      }
    },
  );
  server.requestTimeout = 15000;
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  test = spawn(
    "cargo",
    [
      "test",
      "--manifest-path",
      "tools/agent/Cargo.toml",
      "--locked",
      "--test",
      "git_https",
      "--",
      "--ignored",
      "--nocapture",
    ],
    {
      cwd: root,
      env: {
        ...env,
        NEWPORT_GIT_HTTPS_URL: `https://127.0.0.1:${server.address().port}/repo.git`,
        NEWPORT_GIT_HTTPS_CERT: cert,
      },
      stdio: "inherit",
    },
  );
  const timer = setTimeout(() => test.kill("SIGKILL"), 120000);
  let code;
  try {
    code = await new Promise((resolve, reject) => {
      test.once("error", reject);
      test.once("exit", resolve);
    });
  } finally {
    clearTimeout(timer);
  }
  if (code !== 0) throw Error(`HTTPS contract test failed (${code})`);
  if (authenticated < 2 || denied < 1)
    throw Error("Authentication path was not exercised");
  console.log(
    `PASS: ${authenticated} authenticated requests; repository TLS and credential configuration honored`,
  );
} finally {
  test?.kill();
  for (const child of backends) child.kill("SIGKILL");
  if (server) {
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
  }
  rmSync(temp, { recursive: true, force: true });
}
