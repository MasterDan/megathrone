import { createResource } from "solid-js";
import { invoke } from "@tauri-apps/api/core";

export function useSingBoxVersion() {
  const [version, { refetch }] = createResource(() => invoke<string>("sing_box_version"));

  return { version, refetch };
}
