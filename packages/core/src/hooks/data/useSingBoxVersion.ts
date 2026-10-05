import { createResource } from "solid-js";

import { usePlatform } from "@/platform/context";

export function useSingBoxVersion() {
  const platform = usePlatform();
  const [version, { refetch }] = createResource(() =>
    platform.invoke<string>("sing_box_version"),
  );

  return { version, refetch };
}
