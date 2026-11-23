// Keep anchor destinations below the navigation when it wraps on small screens.
const topbar = document.querySelector<HTMLElement>(".topbar");
if (topbar) {
  new ResizeObserver(() => {
    document.documentElement.style.setProperty("--nav-height", `${topbar.offsetHeight + 16}px`);
  }).observe(topbar);
}

// Highlight the current section in the docs sidebar as the reader scrolls.
const outlineLinks = [...document.querySelectorAll<HTMLAnchorElement>(".docs-nav ol a[href^='#']")];
if (outlineLinks.length > 0) {
  const targets = outlineLinks
    .map((a) => {
      const el = document.getElementById(a.getAttribute("href")!.slice(1));
      return el ? { a, el } : null;
    })
    .filter((x): x is { a: HTMLAnchorElement; el: HTMLElement } => x !== null);

  let active: (typeof targets)[number] | undefined;
  let scheduled = false;
  const updateCurrent = () => {
    scheduled = false;
    const threshold = (topbar?.offsetHeight ?? 56) + 48;
    let next = targets[0];
    for (const item of targets) {
      if (item.el.getBoundingClientRect().top <= threshold) next = item;
    }
    if (next === active) return;
    active?.a.removeAttribute("aria-current");
    next?.a.setAttribute("aria-current", "location");
    active = next;
  };
  const onScroll = () => {
    if (scheduled) return;
    scheduled = true;
    requestAnimationFrame(updateCurrent);
  };
  window.addEventListener("scroll", onScroll, { passive: true });
  updateCurrent();
}

// Close the compact mobile docs menu when an in-page section link is clicked.
const mobileNav = document.querySelector<HTMLDetailsElement>(".docs-mobile-nav");
if (mobileNav) {
  mobileNav.addEventListener("click", (e) => {
    if (e.target instanceof Element && e.target.closest("a[href^='#']")) mobileNav.open = false;
  });
}

// Add a Copy button to full command-line blocks in the docs.
// Older browsers without :has() reject the whole selector, so ask for it only where it works.
const cmdBlocks = CSS.supports("selector(:has(> *))") ? ".prose .cmd, .prose p:has(> code.cmd)" : ".prose .cmd";
for (const block of document.querySelectorAll<HTMLElement>(cmdBlocks)) {
  const code = block.querySelector("code");
  if (!code || !navigator.clipboard) continue;
  const btn = document.createElement("button");
  btn.type = "button";
  btn.className = "cmd-copy";
  btn.textContent = "Copy";
  btn.setAttribute("aria-label", "Copy command");
  let resetTimer: number | undefined;
  const scheduleReset = () => {
    clearTimeout(resetTimer);
    resetTimer = window.setTimeout(() => {
      btn.textContent = "Copy";
      btn.setAttribute("aria-label", "Copy command");
      delete btn.dataset.copied;
    }, 1500);
  };
  btn.addEventListener("click", () => {
    void navigator.clipboard
      .writeText(code.textContent ?? "")
      .then(() => {
        btn.textContent = "Copied";
        btn.setAttribute("aria-label", "Copied command");
        btn.dataset.copied = "true";
        scheduleReset();
      })
      .catch(() => {
        btn.textContent = "Couldn't copy";
        btn.setAttribute("aria-label", "Couldn't copy command");
        scheduleReset();
      });
  });
  block.append(btn);
}
