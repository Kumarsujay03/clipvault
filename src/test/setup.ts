import "@testing-library/jest-dom/vitest";

// jsdom lacks these browser APIs used by the app.
if (!window.matchMedia) {
  window.matchMedia = (query: string) =>
    ({
      matches: false,
      media: query,
      addEventListener() {},
      removeEventListener() {},
      onchange: null,
      addListener() {},
      removeListener() {},
      dispatchEvent: () => false,
    }) as MediaQueryList;
}
Element.prototype.scrollTo ||= function () {};
globalThis.ResizeObserver ||= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};

// jsdom has no layout; give elements a viewport-sized box so the virtual list renders rows.
Element.prototype.getBoundingClientRect = function () {
  return { x: 0, y: 0, top: 0, left: 0, right: 600, bottom: 800, width: 600, height: 800, toJSON() {} } as DOMRect;
};
Object.defineProperty(HTMLElement.prototype, "offsetHeight", { configurable: true, get: () => 800 });
Object.defineProperty(HTMLElement.prototype, "offsetWidth", { configurable: true, get: () => 600 });
