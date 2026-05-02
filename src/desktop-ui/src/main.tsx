import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { I18nProvider } from "@va/i18n";
import App from "./App";
import "./index.css";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <I18nProvider>
      <App />
    </I18nProvider>
  </StrictMode>
);

// Fade out the static HTML splash after 1s minimum
const splash = document.getElementById("splash");
if (splash) {
  const elapsed = performance.now();
  const remaining = Math.max(0, 1000 - elapsed);
  setTimeout(() => {
    splash.classList.add("hidden");
    setTimeout(() => splash.remove(), 400);
  }, remaining);
}
