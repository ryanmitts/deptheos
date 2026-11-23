// Wipers: a range input over two stacked images sets --split, where the top image is clipped.
for (const el of document.querySelectorAll<HTMLElement>("[data-compare]")) {
  const input = el.querySelector("input");
  if (!input) continue;
  const set = () => {
    const v = Number(input.value);
    el.style.setProperty("--split", `${v}%`);
    el.dataset.near = v < 18 ? "left" : v > 82 ? "right" : "";
  };
  input.addEventListener("input", set);
  set();
}
