/* @refresh reload */
import { render } from "solid-js/web";

import { App } from "@megathrone/core";
import { WebContext } from "./webContext";

import "@megathrone/core/index.css";

const mountPoint = document.getElementById("root");

render(() => (
  <WebContext>
    <App />
  </WebContext>
), mountPoint!);
