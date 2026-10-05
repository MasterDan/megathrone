/* @refresh reload */
import { render } from "solid-js/web";

import { App } from "@megathrone/core";
import { TauriContext } from "./tauriContext";

import "@megathrone/core/index.css";

const mountPoint = document.getElementById("root");

render(() => (
  <TauriContext>
    <App />
  </TauriContext>
), mountPoint!);
