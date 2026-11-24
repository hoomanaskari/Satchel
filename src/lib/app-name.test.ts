import { describe, expect, it } from "vitest";
import { APP_NAME, appName } from "./app-name";

describe("appName", () => {
  it("names Satchel Dev in the development modes and Satchel otherwise", () => {
    expect(appName("dev-app")).toBe("Satchel Dev");
    expect(appName("development")).toBe("Satchel Dev");
    expect(appName("production")).toBe("Satchel");
    expect(appName("test")).toBe("Satchel");
    expect(APP_NAME).toBe("Satchel");
  });
});
