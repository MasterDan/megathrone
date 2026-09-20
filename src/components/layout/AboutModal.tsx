import type { Accessor, Component } from "solid-js";
import { Match, Switch } from "solid-js";
import { TbOutlineAlertTriangle, TbOutlineRouter } from "solid-icons/tb";

import { CrownMark } from "@/components/common/CrownMark";
import { Modal } from "@/components/common/daisy-ui/Modal";
import { useSingBoxVersion } from "@/hooks/data/useSingBoxVersion";

export const AboutModal: Component<{
  opened: Accessor<boolean>;
  setOpened: (value: boolean) => void;
}> = (props) => {
  const { version } = useSingBoxVersion();

  return (
    <Modal
      opened={props.opened}
      setOpened={props.setOpened}
      title="About"
      class="max-w-lg"
    >
      <div class="flex flex-col items-center gap-3 py-4 text-center">
        <CrownMark class="size-16 text-primary" />
        <h3 class="text-2xl font-bold">Megathrone</h3>
        <p class="text-base-content/70">Desktop proxy client powered by sing-box.</p>
        <Switch>
          <Match when={version.error}>
            <div class="badge badge-error badge-outline gap-1">
              <TbOutlineAlertTriangle size={14} />
              sing-box unavailable
            </div>
          </Match>
          <Match when={version.loading}>
            <span class="loading loading-spinner loading-sm text-primary" />
          </Match>
          <Match when={version()}>
            <div class="badge badge-outline gap-2 opacity-70">
              <TbOutlineRouter size={14} />
              sing-box {version()}
            </div>
          </Match>
        </Switch>
      </div>
    </Modal>
  );
};
