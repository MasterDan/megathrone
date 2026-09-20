import type { Accessor, Component } from "solid-js";
import { Show, createEffect, createSignal } from "solid-js";
import { open } from "@tauri-apps/plugin-dialog";
import { TbOutlineCloudDownload, TbOutlineClipboard, TbOutlineFileText } from "solid-icons/tb";

import { Button } from "@/components/common/daisy-ui/Button";
import { Modal } from "@/components/common/daisy-ui/Modal";
import { useProfileActions } from "@/hooks/data/useProfileActions";

interface Props {
  opened: Accessor<boolean>;
  setOpened: (value: boolean) => void;
  onDone: () => void;
}

type Tab = "url" | "file" | "text";

export const AddProfileModal: Component<Props> = (props) => {
  const { busy, error, importFromUrl, importFromFile, importFromText } =
    useProfileActions(() => {
      props.setOpened(false);
      props.onDone();
    });

  const [tab, setTab] = createSignal<Tab>("url");
  const [url, setUrl] = createSignal("");
  const [name, setName] = createSignal("");
  const [path, setPath] = createSignal("");
  const [text, setText] = createSignal("");
  const [pasteFailed, setPasteFailed] = createSignal(false);

  // The modal stays mounted for exit animations, so drafts reset on open.
  createEffect(() => {
    if (props.opened()) {
      setTab("url");
      setUrl("");
      setName("");
      setPath("");
      setText("");
      setPasteFailed(false);
    }
  });

  const selectTab = (value: Tab) => {
    setTab(value);
    setPasteFailed(false);
  };

  const pasteFromClipboard = async (setter: (value: string) => void) => {
    setPasteFailed(false);
    try {
      const value = await navigator.clipboard.readText();
      if (value.trim()) {
        setter(value);
        return;
      }
    } catch {
      // clipboard may be unavailable (no focus / permission)
    }
    setPasteFailed(true);
  };

  const chooseFile = async () => {
    const selected = await open({
      multiple: false,
      directory: false,
      filters: [{ name: "Subscription", extensions: ["txt", "json", "yaml", "yml", "conf"] }],
    });
    if (typeof selected === "string") {
      setPath(selected);
      if (!name().trim()) {
        const fileName = selected.split(/[/\\]/).pop() ?? "";
        setName(fileName.replace(/\.[^.]+$/, ""));
      }
    }
  };

  const submit = () => {
    if (busy()) {
      return;
    }
    if (tab() === "url") {
      if (!url().trim()) {
        return;
      }
      void importFromUrl(url().trim(), name());
      return;
    }
    if (tab() === "file") {
      if (!path()) {
        return;
      }
      void importFromFile(path(), name());
      return;
    }
    if (!text().trim() || !name().trim()) {
      return;
    }
    void importFromText(text(), name());
  };

  return (
    <Modal
      opened={props.opened}
      setOpened={props.setOpened}
      title="Add profile"
      class="max-w-lg"
      actions={
        <>
          <Button ghost disabled={busy()} onClick={() => props.setOpened(false)}>
            Cancel
          </Button>
          <Button variant="primary" disabled={busy()} onClick={submit}>
            <Show when={!busy()} fallback={null}>
              <TbOutlineCloudDownload size={18} />
            </Show>
            Import
          </Button>
        </>
      }
    >
      <div class="space-y-3">
        <div role="tablist" class="tabs tabs-boxed">
          <button type="button" role="tab" class={`tab ${tab() === "url" ? "tab-active" : ""}`} onClick={() => selectTab("url")}>
            From URL
          </button>
          <button type="button" role="tab" class={`tab ${tab() === "file" ? "tab-active" : ""}`} onClick={() => selectTab("file")}>
            From file
          </button>
          <button type="button" role="tab" class={`tab ${tab() === "text" ? "tab-active" : ""}`} onClick={() => selectTab("text")}>
            Paste text
          </button>
        </div>

        <label class="form-control">
          <span class="label-text mb-1 block text-sm">Name</span>
          <input
            type="text"
            class="input input-bordered w-full"
            placeholder={
              tab() === "url"
                ? "optional, defaults to the URL host"
                : tab() === "file"
                  ? "optional, defaults to the file name"
                  : "profile name"
            }
            value={name()}
            onInput={(event) => setName(event.currentTarget.value)}
          />
        </label>

        <Show when={tab() === "url"}>
          <div class="form-control">
            <span class="label-text mb-1 block text-sm">Subscription URL</span>
            <div class="relative">
              <input
                type="url"
                class="input input-bordered w-full pr-10"
                placeholder="https://example.com/subscription.txt"
                value={url()}
                onInput={(event) => setUrl(event.currentTarget.value)}
              />
              <button
                type="button"
                class="absolute top-1/2 right-1 flex size-8 -translate-y-1/2 cursor-pointer items-center justify-center rounded-full bg-base-content/10 text-base-content/70 transition-colors hover:bg-base-content/20 hover:text-base-content"
                title="Paste from clipboard"
                onClick={() => void pasteFromClipboard(setUrl)}
              >
                <TbOutlineClipboard size={18} />
              </button>
            </div>
          </div>
        </Show>

        <Show when={tab() === "file"}>
          <div class="form-control space-y-2">
            <span class="label-text block text-sm">Subscription file</span>
            <button
              type="button"
              class="inline-flex h-10 w-full cursor-pointer select-none items-center justify-center gap-2 rounded-full bg-base-content/10 px-4 text-sm font-medium whitespace-nowrap text-base-content transition-colors hover:bg-base-content/20"
              onClick={() => void chooseFile()}
            >
              <TbOutlineFileText size={18} />
              {path() ? "Change file…" : "Choose file…"}
            </button>
            <Show when={path()}>
              <p class="truncate rounded-lg bg-base-200 px-3 py-2 text-xs" title={path()}>
                {path()}
              </p>
            </Show>
            <p class="text-xs text-base-content/50">
              The file path is remembered, so the profile can be refreshed later.
            </p>
          </div>
        </Show>

        <Show when={tab() === "text"}>
          <div class="form-control">
            <div class="mb-1 flex items-center justify-between">
              <span class="label-text block text-sm">Content</span>
              <button
                type="button"
                class="inline-flex h-6 cursor-pointer select-none items-center justify-center gap-1 rounded-full bg-base-content/10 px-2.5 text-xs font-medium whitespace-nowrap text-base-content transition-colors hover:bg-base-content/20"
                onClick={() => void pasteFromClipboard(setText)}
              >
                <TbOutlineClipboard size={14} />
                Paste
              </button>
            </div>
            <textarea
              class="textarea textarea-bordered max-h-40 min-h-24 w-full font-mono text-xs"
              placeholder="vless://…, ss://…, vmess://…, base64 subscription or sing-box JSON"
              value={text()}
              onInput={(event) => setText(event.currentTarget.value)}
            />
            <span class="label-text-alt mt-1 text-base-content/50">
              Pasted profiles have no source and can't be refreshed automatically.
            </span>
          </div>
        </Show>

        <Show when={pasteFailed()}>
          <p class="text-xs text-error">Clipboard is empty or unavailable</p>
        </Show>

        <Show when={error()}>
          <div class="alert alert-error py-2 text-sm">
            <span class="break-all">{error()}</span>
          </div>
        </Show>
      </div>
    </Modal>
  );
};
