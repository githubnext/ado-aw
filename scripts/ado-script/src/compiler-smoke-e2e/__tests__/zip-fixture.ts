/** Test-only zip writer for the artifact-download tests. */
import { deflateRawSync } from "node:zlib";

export interface ZipFixtureEntry {
  name: string;
  data: string;
  deflate?: boolean;
  /** Write zero sizes in the local header, as streaming writers do. */
  dataDescriptor?: boolean;
}

/** Build a minimal zip archive (CRCs are not checked by the reader). */
export function makeZip(entries: ZipFixtureEntry[]): Buffer {
  const locals: Buffer[] = [];
  const centrals: Buffer[] = [];
  let offset = 0;
  for (const entry of entries) {
    const raw = Buffer.from(entry.data, "utf8");
    const stored = entry.deflate ? deflateRawSync(raw) : raw;
    const name = Buffer.from(entry.name, "utf8");
    const local = Buffer.alloc(30);
    local.writeUInt32LE(0x04034b50, 0);
    local.writeUInt16LE(entry.dataDescriptor ? 0x8 : 0, 6);
    local.writeUInt16LE(entry.deflate ? 8 : 0, 8);
    local.writeUInt32LE(entry.dataDescriptor ? 0 : stored.length, 18);
    local.writeUInt32LE(entry.dataDescriptor ? 0 : raw.length, 22);
    local.writeUInt16LE(name.length, 26);
    locals.push(local, name, stored);

    const central = Buffer.alloc(46);
    central.writeUInt32LE(0x02014b50, 0);
    central.writeUInt16LE(entry.dataDescriptor ? 0x8 : 0, 8);
    central.writeUInt16LE(entry.deflate ? 8 : 0, 10);
    central.writeUInt32LE(stored.length, 20);
    central.writeUInt32LE(raw.length, 24);
    central.writeUInt16LE(name.length, 28);
    central.writeUInt32LE(offset, 42);
    centrals.push(central, name);
    offset += 30 + name.length + stored.length;
  }
  const centralDir = Buffer.concat(centrals);
  const eocd = Buffer.alloc(22);
  eocd.writeUInt32LE(0x06054b50, 0);
  eocd.writeUInt16LE(entries.length, 8);
  eocd.writeUInt16LE(entries.length, 10);
  eocd.writeUInt32LE(centralDir.length, 12);
  eocd.writeUInt32LE(offset, 16);
  return Buffer.concat([...locals, centralDir, eocd]);
}
