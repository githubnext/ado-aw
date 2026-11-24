/**
 * Minimal, dependency-free reader for the zip archives Azure DevOps serves as
 * build-artifact downloads.
 *
 * Supports exactly what the smoke harness needs: locate one entry by its
 * basename via the central directory and return its bytes (stored or
 * deflated). Sizes are always read from the central directory, so archives
 * whose local headers defer sizes to a data descriptor are handled. ZIP64
 * and encrypted entries are rejected rather than misread.
 *
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import { inflateRawSync } from "node:zlib";

const EOCD_SIGNATURE = 0x06054b50;
const CENTRAL_SIGNATURE = 0x02014b50;
const LOCAL_SIGNATURE = 0x04034b50;
const EOCD_MIN_SIZE = 22;
const MAX_COMMENT = 0xffff;

function findEndOfCentralDirectory(zip: Buffer): number {
  const lowest = Math.max(0, zip.length - EOCD_MIN_SIZE - MAX_COMMENT);
  for (let offset = zip.length - EOCD_MIN_SIZE; offset >= lowest; offset--) {
    if (zip.readUInt32LE(offset) === EOCD_SIGNATURE) return offset;
  }
  throw new Error("not a zip archive (end of central directory not found)");
}

/** Every entry name in the archive's central directory, in archive order. */
export function listZipEntries(zip: Buffer): string[] {
  return readCentralDirectory(zip).map((entry) => entry.name);
}

interface CentralEntry {
  name: string;
  method: number;
  flags: number;
  compressedSize: number;
  uncompressedSize: number;
  localOffset: number;
}

function readCentralDirectory(zip: Buffer): CentralEntry[] {
  const eocd = findEndOfCentralDirectory(zip);
  const count = zip.readUInt16LE(eocd + 10);
  let offset = zip.readUInt32LE(eocd + 16);
  if (count === 0xffff || offset === 0xffffffff) {
    throw new Error("ZIP64 archives are not supported");
  }
  const entries: CentralEntry[] = [];
  for (let i = 0; i < count; i++) {
    if (offset + 46 > zip.length || zip.readUInt32LE(offset) !== CENTRAL_SIGNATURE) {
      throw new Error(`corrupt zip central directory at entry ${i}`);
    }
    const nameLength = zip.readUInt16LE(offset + 28);
    const extraLength = zip.readUInt16LE(offset + 30);
    const commentLength = zip.readUInt16LE(offset + 32);
    entries.push({
      flags: zip.readUInt16LE(offset + 8),
      method: zip.readUInt16LE(offset + 10),
      compressedSize: zip.readUInt32LE(offset + 20),
      uncompressedSize: zip.readUInt32LE(offset + 24),
      localOffset: zip.readUInt32LE(offset + 42),
      name: zip.toString("utf8", offset + 46, offset + 46 + nameLength),
    });
    offset += 46 + nameLength + extraLength + commentLength;
  }
  return entries;
}

/**
 * Return the bytes of the single entry whose basename is `fileName`, or
 * `undefined` when no entry matches. Throws when more than one entry
 * matches, so an unexpected archive layout is never silently resolved.
 */
export function readZipEntryByBasename(zip: Buffer, fileName: string): Buffer | undefined {
  const matches = readCentralDirectory(zip).filter(
    (entry) => !entry.name.endsWith("/") && entry.name.split("/").pop() === fileName,
  );
  if (matches.length === 0) return undefined;
  if (matches.length > 1) {
    throw new Error(
      `zip contains ${matches.length} entries named '${fileName}': ${matches.map((m) => m.name).join(", ")}`,
    );
  }
  const entry = matches[0]!;
  if (entry.flags & 0x1) throw new Error(`zip entry '${entry.name}' is encrypted`);
  const local = entry.localOffset;
  if (local + 30 > zip.length || zip.readUInt32LE(local) !== LOCAL_SIGNATURE) {
    throw new Error(`corrupt zip local header for '${entry.name}'`);
  }
  const dataStart = local + 30 + zip.readUInt16LE(local + 26) + zip.readUInt16LE(local + 28);
  const data = zip.subarray(dataStart, dataStart + entry.compressedSize);
  if (data.length !== entry.compressedSize) {
    throw new Error(`zip entry '${entry.name}' is truncated`);
  }
  let content: Buffer;
  if (entry.method === 0) content = Buffer.from(data);
  else if (entry.method === 8) {
    content = inflateRawSync(data, { maxOutputLength: Math.max(1, entry.uncompressedSize) });
  }
  else throw new Error(`zip entry '${entry.name}' uses unsupported compression method ${entry.method}`);
  if (content.length !== entry.uncompressedSize) {
    throw new Error(
      `zip entry '${entry.name}' inflated to ${content.length} bytes, expected ${entry.uncompressedSize}`,
    );
  }
  return content;
}
