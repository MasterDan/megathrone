import type { Component } from "solid-js";

export const CrownMark: Component<{ class?: string }> = (props) => (
  <svg
    viewBox="256 312 512 440"
    class={props.class}
    fill="none"
    xmlns="http://www.w3.org/2000/svg"
    aria-hidden="true"
  >
    <g stroke="currentColor" stroke-width="32" stroke-linecap="round" stroke-linejoin="round">
      <path d="M315 455 V627" />
      <path d="M707 455 V627" />
      <path d="M315 455 L414 580 L513 372 L609 580 L707 455" />
    </g>
    <g fill="currentColor">
      <circle cx="315" cy="627" r="34" />
      <circle cx="707" cy="627" r="34" />
      <circle cx="414" cy="580" r="25" />
      <circle cx="609" cy="580" r="25" />
      <circle cx="315" cy="455" r="44" />
      <circle cx="707" cy="455" r="44" />
      <circle cx="513" cy="372" r="48" />
      <rect x="289" y="681" width="447" height="60" rx="30" />
    </g>
    <g fill="#6d28d9">
      <circle cx="315" cy="455" r="17" />
      <circle cx="707" cy="455" r="17" />
      <circle cx="513" cy="372" r="19" />
    </g>
  </svg>
);
