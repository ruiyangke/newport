import { FileIcon as SymbolsFileIcon } from "@react-symbols/icons/utils";
import { Rust, Docker } from "@react-symbols/icons/files";
import { File, FileKey2, Folder, FolderOpen, Link } from "lucide-react";
import type { FileEntry } from "../domain/files";

const fileNames = {
  "cargo.toml": Rust,
  "cargo.lock": Rust,
  containerfile: Docker,
};

/** Filename hints are presentation only; previews still inspect actual bytes. */
export function FileIcon({
  name,
  kind,
  open = false,
  size = 16,
}: {
  name: string;
  kind: FileEntry["kind"];
  open?: boolean;
  size?: number;
}) {
  const lower = name.toLowerCase();
  const props = {
    width: size,
    height: size,
    "aria-hidden": true as const,
    focusable: false as const,
  };
  if (kind === "directory")
    return open ? <FolderOpen {...props} /> : <Folder {...props} />;
  if (kind === "symlink") return <Link {...props} />;
  if (kind !== "file") return <File {...props} />;
  if (
    /^id_(rsa|ed25519|ecdsa|dsa)(\.pub)?$/.test(lower) ||
    lower === "authorized_keys" ||
    lower === "known_hosts"
  ) {
    return <FileKey2 {...props} />;
  }
  return (
    <SymbolsFileIcon
      {...props}
      fileName={lower.startsWith(".env.") ? ".env" : lower}
      autoAssign
      editFileNameData={fileNames}
    />
  );
}
