import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { CookieGuide, cookieGuideProvider } from "./components/CookieGuide";
import "./index.css";

document.documentElement.classList.toggle(
  "dark",
  localStorage.getItem("token-show-theme") !== "light",
);

const guide = cookieGuideProvider();

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    {guide ? <CookieGuide provider={guide} /> : <App />}
  </React.StrictMode>,
);
