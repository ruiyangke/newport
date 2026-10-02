import { cn } from "../lib/utils";
import {
  EditorBody,
  EditorSection,
  EditorFooter,
  EditorCheckboxField,
} from "./EditorLayout";
import { SectionActions } from "./SectionHeader";
import type { AgentKey } from "../api/desktop";
import { Label } from "./ui/label";
import { Dialog, DialogContent, DialogTitle } from "./ui/dialog";
import { Checkbox, Button, SelectItem } from "./controls";
import { EditorInput as Input, EditorSelect as Select } from "./EditorControls";
import { desktop } from "../api/desktop";
import {
  useEffect,
  useRef,
  useState,
  type FormEvent,
  type ReactNode,
} from "react";
import { ArrowRight, X } from "lucide-react";
import { parseRange, range, type Server, type Tunnel } from "../types";

export function Modal({
  title,
  children,
  onClose,
  describedBy,
  busy = false,
  className = "",
}: {
  title: string;
  children: ReactNode;
  onClose: () => void;
  describedBy?: string;
  busy?: boolean;
  className?: string;
}) {
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !busy) onClose();
      }}
    >
      <DialogContent
        className={cn(
          "flex w-[480px] max-w-[calc(100vw-32px)] flex-col gap-0 overflow-hidden p-0 sm:max-w-[calc(100vw-32px)]",
          className,
        )}
        showCloseButton={false}
        aria-describedby={describedBy}
        aria-busy={busy}
      >
        <header
          data-slot="dialog-header"
          className="flex shrink-0 items-center justify-between gap-[12px] px-[24px] pt-[20px] pb-[12px] [@media(max-height:560px),(max-width:540px)]:px-[18px] [@media(max-height:560px),(max-width:540px)]:pt-[14px] [@media(max-height:560px),(max-width:540px)]:pb-[8px]"
        >
          <DialogTitle className="text-[17px] font-semibold">
            {title}
          </DialogTitle>
          <Button
            variant="ghost"
            size="icon"
            className="shrink-0 text-muted-foreground"
            disabled={busy}
            onClick={onClose}
            aria-label="Close dialog"
            title=""
          >
            <X size={18} />
          </Button>
        </header>
        {children}
      </DialogContent>
    </Dialog>
  );
}

function Field({
  label,
  children,
  hint,
}: {
  label: string;
  children: ReactNode;
  hint?: string;
}) {
  return (
    <Label
      data-slot="field"
      className="flex min-w-0 flex-col items-stretch gap-[7px] text-left text-[12px]"
    >
      <span className="text-[12px] font-medium">{label}</span>
      {children}
      {hint && <small className="text-[11px] font-normal">{hint}</small>}
    </Label>
  );
}
function KeyPicker({
  server,
  onChange,
}: {
  server: Server;
  onChange: (server: Server) => void;
}) {
  const [keys, setKeys] = useState<AgentKey[]>([]);
  const [warnings, setWarnings] = useState<string[]>([]);
  const [loading, setLoading] = useState(false);
  const [fileMode, setFileMode] = useState(Boolean(server.identityFile));
  const [revision, setRevision] = useState(0);
  useEffect(() => {
    let disposed = false;
    setLoading(true);
    desktop("ssh_agent_keys")
      .then((result) => {
        if (!disposed) {
          setKeys(result.keys);
          setWarnings(result.warnings);
        }
      })
      .catch((error) => {
        if (!disposed)
          setWarnings([`Could not read agent keys: ${String(error)}`]);
      })
      .finally(() => {
        if (!disposed) setLoading(false);
      });
    return () => {
      disposed = true;
    };
  }, [revision]);
  const keyId = (key: Pick<AgentKey, "source" | "fingerprint">) =>
    `${key.source}:${key.fingerprint}`;
  const selected = server.agentKeyFingerprint
    ? `${server.agentSource ?? "system"}:${server.agentKeyFingerprint}`
    : fileMode
      ? "file"
      : "automatic";
  return (
    <>
      <Field label="SSH key">
        <Select
          aria-label="SSH key"
          value={selected}
          onValueChange={(value) => {
            const key = keys.find((key) => keyId(key) === value);
            setFileMode(value === "file");
            onChange({
              ...server,
              identityFile: value === "file" ? server.identityFile : null,
              agentSource: key?.source ?? null,
              agentKeyFingerprint: key?.fingerprint ?? null,
            });
          }}
        >
          <SelectItem value="automatic">
            Automatic · system agent and default keys
          </SelectItem>
          <SelectItem value="file">Enter a key-file path…</SelectItem>
          {server.agentKeyFingerprint &&
            !keys.some((key) => keyId(key) === selected) && (
              <SelectItem value={selected}>
                Saved key ·{" "}
                {loading ? "checking agent…" : "not currently available"}
              </SelectItem>
            )}
          {keys.map((key) => (
            <SelectItem key={keyId(key)} value={keyId(key)}>
              {key.source === "onePassword" ? "1Password" : "System agent"} ·{" "}
              {key.comment || key.algorithm} · {key.fingerprint}
            </SelectItem>
          ))}
        </Select>
      </Field>
      <SectionActions>
        <Button
          type="button"
          loading={loading}
          onClick={() => setRevision((n) => n + 1)}
        >
          {loading ? "Reading agent keys…" : "Refresh agent keys"}
        </Button>
      </SectionActions>
      {!loading && !keys.length && (
        <p className="text-[12px] leading-[1.65] text-muted-foreground">
          No keys found. Unlock your SSH agent and refresh, or use a key file.
        </p>
      )}
      {warnings.map((warning) => (
        <p
          className="text-[12px] leading-[1.65] text-muted-foreground"
          key={warning}
        >
          {warning}
        </p>
      ))}
      {server.agentKeyFingerprint && (
        <p className="text-[12px] leading-[1.65] text-muted-foreground wrap-anywhere">
          Selected key: {server.agentKeyFingerprint}
        </p>
      )}
      {fileMode && !server.agentKeyFingerprint && (
        <Field
          label="Identity file"
          hint="Load encrypted keys into your system agent first."
        >
          <Input
            required
            value={server.identityFile ?? ""}
            onChange={(event) =>
              onChange({ ...server, identityFile: event.target.value })
            }
            placeholder="~/.ssh/id_ed25519"
            spellCheck={false}
          />
        </Field>
      )}
    </>
  );
}
export function ServerForm({
  value,
  existing,
  onSave,
  onClose,
}: {
  value: Server;
  existing: boolean;
  onSave: (s: Server, password: string | null) => Promise<void>;
  onClose: () => void;
}) {
  const [s, set] = useState(value);
  const [password, setPassword] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const submitting = useRef(false);
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (submitting.current) return;
    submitting.current = true;
    setBusy(true);
    setError("");
    try {
      await onSave(
        {
          ...s,
          name: s.name.trim(),
          sshHost: s.sshHost.trim(),
          sshUser: s.sshUser.trim(),
          identityFile: s.identityFile?.trim() || null,
        },
        password || null,
      );
    } catch (e) {
      setError(String(e));
    } finally {
      submitting.current = false;
      setBusy(false);
    }
  };
  return (
    <Modal
      title={existing ? "Edit server" : "Add server"}
      className="server-editor"
      busy={busy}
      onClose={onClose}
    >
      <form className="flex min-h-0 flex-col" onSubmit={submit}>
        <EditorBody>
          <EditorSection>
            <legend>Connection</legend>
            <Field label="Name (optional)">
              <Input
                autoFocus
                value={s.name}
                onChange={(e) => set({ ...s, name: e.target.value })}
                placeholder="Development server"
              />
            </Field>
            <div className="grid grid-cols-[minmax(0,1fr)_95px] gap-[14px]">
              <Field label="SSH host">
                <Input
                  required
                  value={s.sshHost}
                  onChange={(e) => set({ ...s, sshHost: e.target.value })}
                  placeholder="server.example.com"
                  spellCheck={false}
                />
              </Field>
              <Field label="SSH port">
                <Input
                  required
                  type="number"
                  min="1"
                  max="65535"
                  value={s.sshPort}
                  onChange={(e) =>
                    set({ ...s, sshPort: Number(e.target.value) })
                  }
                />
              </Field>
            </div>
            <Field label="Username">
              <Input
                required
                value={s.sshUser}
                onChange={(e) => set({ ...s, sshUser: e.target.value })}
                placeholder="username"
                autoCapitalize="off"
                spellCheck={false}
              />
            </Field>
          </EditorSection>
          <EditorSection>
            <legend>Sign in</legend>
            <Field label="Authentication">
              <Select
                aria-label="Authentication"
                value={s.authMethod}
                onValueChange={(value) =>
                  set({
                    ...s,
                    authMethod: value as Server["authMethod"],
                  })
                }
              >
                <SelectItem value="publicKey">SSH key / agent</SelectItem>
                <SelectItem value="password">Password</SelectItem>
              </Select>
            </Field>
            {s.authMethod === "publicKey" ? (
              <KeyPicker server={s} onChange={set} />
            ) : (
              <Field
                label="Password"
                hint={
                  existing
                    ? "Leave empty to keep the saved password."
                    : "Saved in your encrypted vault."
                }
              >
                <Input
                  type="password"
                  required={!existing}
                  autoComplete="new-password"
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                />
              </Field>
            )}
          </EditorSection>
          {error && (
            <p role="alert" className="text-(--red)">
              {error}
            </p>
          )}
        </EditorBody>
        <EditorFooter>
          <Button type="button" disabled={busy} onClick={onClose}>
            Cancel
          </Button>
          <Button type="submit" variant="default" loading={busy}>
            {busy ? "Saving…" : "Save server"}
          </Button>
        </EditorFooter>
      </form>
    </Modal>
  );
}
export function TunnelForm({
  value,
  existing,
  onSave,
  onClose,
}: {
  value: Tunnel;
  existing: boolean;
  onSave: (t: Tunnel) => Promise<void>;
  onClose: () => void;
}) {
  const [t, set] = useState(value);
  const [local, setLocal] = useState(
    range(t.localPort, t.localPortEnd).replace("–", "-"),
  );
  const [remote, setRemote] = useState(
    range(t.remotePort, t.remotePortEnd).replace("–", "-"),
  );
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const submitting = useRef(false);
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (submitting.current) return;
    submitting.current = true;
    setError("");
    setBusy(true);
    try {
      const [lp, le] = parseRange(local),
        [rp, re] = parseRange(remote);
      if ((le ?? lp) - lp !== (re ?? rp) - rp)
        throw new Error(
          "Local and remote ranges must contain the same number of ports.",
        );
      await onSave({
        ...t,
        name: t.name.trim(),
        remoteHost: t.remoteHost.trim(),
        localPort: lp,
        localPortEnd: le,
        remotePort: rp,
        remotePortEnd: re,
      });
    } catch (e) {
      setError(String(e));
    } finally {
      submitting.current = false;
      setBusy(false);
    }
  };
  return (
    <Modal
      title={existing ? "Edit tunnel" : "Add tunnel"}
      busy={busy}
      onClose={onClose}
    >
      <form className="flex min-h-0 flex-col" onSubmit={submit}>
        <EditorBody>
          <Field label="Name">
            <Input
              autoFocus
              value={t.name}
              onChange={(e) => set({ ...t, name: e.target.value })}
              placeholder="Web application"
            />
          </Field>
          <div className="grid grid-cols-[minmax(0,1fr)_18px_minmax(0,1fr)] items-center gap-[12px] [&>svg]:mt-[20px] [&>svg]:text-muted-foreground">
            <Field label="Local port or range">
              <Input
                required
                value={local}
                onChange={(e) => setLocal(e.target.value)}
                placeholder="8000 or 8000-8010"
              />
            </Field>
            <ArrowRight size={18} />
            <Field label="Remote port or range">
              <Input
                required
                value={remote}
                onChange={(e) => setRemote(e.target.value)}
                placeholder="8000 or 8000-8010"
              />
            </Field>
          </div>
          <Field
            label="Remote host"
            hint="Relative to the SSH server. Local ports are accessible only on this computer."
          >
            <Input
              required
              value={t.remoteHost}
              onChange={(e) => set({ ...t, remoteHost: e.target.value })}
              spellCheck={false}
            />
          </Field>
          <EditorCheckboxField>
            <Checkbox
              checked={t.autoConnect}
              onCheckedChange={(checked) =>
                set({ ...t, autoConnect: checked === true })
              }
            />
            <span>Connect when Newport starts</span>
          </EditorCheckboxField>
          <EditorCheckboxField>
            <Checkbox
              checked={t.autoReconnect}
              onCheckedChange={(checked) =>
                set({ ...t, autoReconnect: checked === true })
              }
            />
            <span>
              Reconnect if the connection drops
              <small>Retries up to 10 times.</small>
            </span>
          </EditorCheckboxField>
          {error && (
            <p role="alert" className="text-(--red)">
              {error}
            </p>
          )}
        </EditorBody>
        <EditorFooter>
          <Button type="button" disabled={busy} onClick={onClose}>
            Cancel
          </Button>
          <Button type="submit" variant="default" loading={busy}>
            {busy ? "Saving…" : "Save tunnel"}
          </Button>
        </EditorFooter>
      </form>
    </Modal>
  );
}
