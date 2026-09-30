import { expect, it } from "vitest";
import { gitErrorMessage } from "./errors";

it("explains interruption recovery without exposing RPC instructions", () => {
  expect(
    gitErrorMessage({
      code: "OUTCOME_UNKNOWN",
      message: "Query operation.get with opaque ID",
    }),
  ).toBe(
    "The connection was interrupted before the result was confirmed. Check the saved outcome before trying again.",
  );
  expect(
    gitErrorMessage({
      code: "RECOVERY_REQUIRED",
      message: "Inspect operation opaque-id",
    }),
  ).toBe(
    "An interrupted operation needs review before more changes can be made. Check its saved outcome below.",
  );
});

it("formats only public error messages and codes", () => {
  expect(
    gitErrorMessage({
      code: "INVALID_REQUEST",
      message: "Unsupported branch filter",
      details: { private: "hidden" },
    }),
  ).toBe("Unsupported branch filter (INVALID_REQUEST)");
  expect(gitErrorMessage(new Error("Connection closed"))).toBe(
    "Connection closed",
  );
  expect(gitErrorMessage("Connection closed")).toBe("Connection closed");
  expect(gitErrorMessage({ code: "INVALID_REQUEST" })).toBe(
    "The Git operation failed. (INVALID_REQUEST)",
  );
  expect(gitErrorMessage({ message: { private: "hidden" } })).toBe(
    "The Git operation failed.",
  );
  expect(gitErrorMessage(null)).toBe("The Git operation failed.");
});
