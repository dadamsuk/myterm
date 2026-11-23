import { mount } from "svelte";
import "./app.css";
import App from "./App.svelte";

// The webview's own context menu has nothing useful for a terminal.
window.addEventListener("contextmenu", (e) => e.preventDefault());

export default mount(App, { target: document.getElementById("app")! });
