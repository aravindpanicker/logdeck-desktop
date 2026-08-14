import React from "react";
import ReactDOM from "react-dom/client";
// Base layer first: tokens and the reset must precede any component stylesheet
// in the emitted CSS, since component rules are written to build on them.
import "./styles/global.css";
import App from "./App";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
