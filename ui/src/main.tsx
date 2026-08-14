import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
// Both, for the duration of the Tailwind port: tailwind.css contributes layered
// utilities, styles.css the (unlayered, therefore winning) tokens and component
// rules. styles.css goes away when the last component is converted.
import "./tailwind.css";
import "./styles.css";
import "@xyflow/react/dist/style.css";
import "@xterm/xterm/css/xterm.css";
import { initTheme } from "./theme";

// Apply the saved theme before first paint (avoids a flash of the default).
initTheme();

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
