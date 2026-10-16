import { afterEach, describe, expect, it, vi } from "vitest";
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { defaultSettings } from "../lib/mock";
import { validate } from "../components/SettingsView";
import { sanitizeHtml } from "../components/PreviewView";
import { formatBytes, relativeTime, typeLabel } from "../lib/format";

// Outside Tauri the app runs on the in-memory preview backend (src/lib/mock.ts).

let cleanup: (() => void) | undefined;
afterEach(() => cleanup?.());

// Fresh modules per test: a new store and a new in-memory backend. React and Testing Library
// are re-imported too so everything shares one React instance.
async function renderApp(seed?: "empty") {
  vi.resetModules();
  if (seed === "empty") {
    vi.doMock("../lib/api", async () => {
      const mock = await import("../lib/mock");
      return { api: mock.createMockApi([]), isTauri: false, errorMessage: (e: unknown) => String(e) };
    });
  } else {
    vi.doUnmock("../lib/api");
  }
  const rtl = await import("@testing-library/react");
  const { default: App } = await import("../App");
  const { useApp } = await import("../store");
  cleanup = rtl.cleanup;
  rtl.render(<App />);
  if (seed !== "empty") await waitFor(() => expect(options().length).toBeGreaterThan(0));
  return useApp;
}

const list = () => screen.getByRole("listbox", { name: "Clipboard history" });
const options = () => within(list()).getAllByRole("option");
const selected = () => options().find((o) => o.getAttribute("aria-selected") === "true");

describe("Clipboard panel (preview backend)", () => {
  it("shows demo data, labelled, newest first", async () => {
    await renderApp();
    expect(screen.getByText("Preview — demo data")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Clipboard" })).toBeInTheDocument();
    expect(options()[0]).toHaveTextContent("meeting notes");
    expect(within(options()[1]).getByText("rust")).toBeInTheDocument();
  });

  it("shows the Win+V style empty state", async () => {
    await renderApp("empty");
    expect(await screen.findByText("Nothing here")).toBeInTheDocument();
    expect(screen.getByText(/once you've copied something/)).toBeInTheDocument();
  });

  it("navigates with arrows, previews with Space and returns with Esc", async () => {
    await renderApp();
    const user = userEvent.setup();
    await user.keyboard("{ArrowDown}");
    expect(selected()).toHaveTextContent("ClipVault::open");
    (document.activeElement as HTMLElement | null)?.blur();
    await user.keyboard(" ");
    expect(await screen.findByText(/vault\.len\(\)/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Paste/ })).toBeInTheDocument();
    await user.keyboard("{Escape}");
    await waitFor(() => expect(options().length).toBeGreaterThan(1));
  });

  it("filters by search text and clears with Esc", async () => {
    await renderApp();
    const user = userEvent.setup();
    await user.type(screen.getByLabelText("Search clipboard history"), "example");
    await waitFor(() => expect(options()).toHaveLength(1));
    await user.keyboard("{Escape}");
    await waitFor(() => expect(options().length).toBeGreaterThan(1));
  });

  it("pins with Ctrl+P and shows it under Pinned", async () => {
    await renderApp();
    const user = userEvent.setup();
    await user.keyboard("{Control>}p{/Control}");
    await user.click(screen.getByRole("tab", { name: "Pinned" }));
    await waitFor(() => expect(options()).toHaveLength(2));
  });

  it("deletes the selected item with Delete", async () => {
    const useApp = await renderApp();
    const user = userEvent.setup();
    const before = options().length;
    (document.activeElement as HTMLElement | null)?.blur();
    await user.keyboard("{ArrowDown}{Delete}");
    await waitFor(() => expect(options()).toHaveLength(before - 1));
    expect(useApp.getState().counts?.all).toBe(before - 1);
  });

  it("adds a tag in preview and finds it with #tag search", async () => {
    await renderApp();
    const user = userEvent.setup();
    (document.activeElement as HTMLElement | null)?.blur();
    await user.keyboard(" ");
    const input = await screen.findByLabelText("Add tag");
    await user.type(input, "Release Notes{Enter}");
    expect(await screen.findByText("#release-notes")).toBeInTheDocument();
    await user.keyboard("{Escape}");
    await user.type(screen.getByLabelText("Search clipboard history"), "#release");
    await waitFor(() => expect(options()).toHaveLength(1));
  });

  it("opens settings and applies a toggle immediately", async () => {
    const useApp = await renderApp();
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Settings" }));
    const sw = await screen.findByRole("switch", { name: /Show icon in the top bar/ });
    expect(sw).toBeChecked();
    await user.click(sw);
    await waitFor(() => expect(useApp.getState().settings?.show_tray_icon).toBe(false));
  });
});

describe("dragging the panel", () => {
  it("starts a window move from the title strip but not from its buttons", async () => {
    await renderApp();
    const { api } = await import("../lib/api");
    const spy = vi.spyOn(api, "startMove");
    const { fireEvent } = await import("@testing-library/react");
    const strip = screen.getAllByTitle("Clipboard")[0];
    fireEvent.mouseDown(strip, { button: 0 });
    expect(spy).toHaveBeenCalledTimes(1);
    fireEvent.mouseDown(screen.getByRole("button", { name: "Settings" }), { button: 0 });
    fireEvent.mouseDown(strip, { button: 2 });
    expect(spy).toHaveBeenCalledTimes(1);
  });
});

describe("rich text", () => {
  it("strips scripts, handlers and remote resources", () => {
    const out = sanitizeHtml(
      `<p onclick="x()">hi <b>there</b><script>alert(1)</script><img src="https://e.com/t.png" onerror="y()"><a href="javascript:z()">l</a></p>`,
    );
    expect(out).toContain("<b>there</b>");
    expect(out).not.toMatch(/script|onclick|onerror|<img|javascript:/);
  });
});

describe("settings validation", () => {
  it("accepts defaults and rejects out-of-range values", () => {
    expect(validate(defaultSettings)).toEqual({});
    expect(validate({ ...defaultSettings, max_items: 5 }).max_items).toBeDefined();
    expect(validate({ ...defaultSettings, thumbnail_size: NaN }).thumbnail_size).toBeDefined();
    expect(validate({ ...defaultSettings, retention_days: 1.5 }).retention_days).toBeDefined();
  });
});

describe("formatting", () => {
  it("formats sizes, times and labels", () => {
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(1536)).toBe("1.5 KB");
    expect(formatBytes(20 * 1024 * 1024)).toBe("20 MB");
    const now = Date.now();
    expect(relativeTime(now - 10_000, now)).toBe("just now");
    expect(relativeTime(now - 5 * 60_000, now)).toBe("5 min ago");
    expect(relativeTime(now - 26 * 3600_000, now)).toBe("yesterday");
    expect(typeLabel({ itemType: "files", fileCount: 3 })).toBe("3 files");
  });
});
