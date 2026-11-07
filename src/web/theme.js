// The page's appearance, set before anything is drawn so it never flashes the other one: Light,
// Dark, or System (the device's choice, followed as it changes), as chosen in this browser under
// Your account (appearance.js). Loaded as a plain script in the page's head, ahead of the
// styles; app.css's light palette applies under `data-theme="light"`, its dark one otherwise.
(() => {
  const KEY = "melete.appearance";
  const system = matchMedia("(prefers-color-scheme: light)");
  const chosen = () => {
    try {
      const kept = localStorage.getItem(KEY);
      return kept === "light" || kept === "dark" ? kept : "system";
    } catch {
      // Storage refused (a private window, say): the device's choice.
      return "system";
    }
  };
  const apply = () => {
    const which = chosen();
    const light = which === "light" || (which === "system" && system.matches);
    document.documentElement.dataset.theme = light ? "light" : "dark";
  };
  apply();
  system.addEventListener("change", apply);
  // Chosen here (appearance.js says so) or in another tab of this browser.
  addEventListener("melete:appearance", apply);
  addEventListener("storage", (event) => {
    if (event.key === KEY || event.key === null) apply();
  });
})();
