<?php
/**
 * tfa_extract.php — extract a Trove .tfa archive using its .tfi index.
 *
 * A Trove asset directory (e.g. "ui/") holds one index .tfi plus one or more
 * archiveN.tfa files. The .tfi is the index: a flat list of entries, each
 * pointing at which archive the file lives in, its byte range inside that
 * archive's DECOMPRESSED content, and a Trove FNV-1a hash of the file. A .tfa
 * is just a zlib stream of all that archive's files concatenated together.
 *
 * This script takes ONE .tfi and ONE .tfa, figures out which archive index the
 * .tfa is (by matching its inflated length to the index), and writes out every
 * file that lives in that archive — preserving relative paths and verifying
 * each file against the size + FNV hash recorded in the index.
 *
 * Usage:
 *   php tfa_extract.php <index.tfi> <archiveN.tfa> [output_dir] [--archive=N] [--list]
 *
 *   output_dir   where to write extracted files (default: ./extracted)
 *   --archive=N  force the archive index instead of auto-detecting it
 *   --list       just list what the .tfi indexes, extract nothing
 *
 * Examples:
 *   php tfa_extract.php index.tfi archive0.tfa
 *   php tfa_extract.php index.tfi archive3.tfa out --archive=3
 *   php tfa_extract.php index.tfi archive0.tfa --list
 *
 * Format per TFI entry (all integers are unsigned LEB128):
 *   leb128 name_len · name_len bytes (UTF-8, may be null-padded) ·
 *   leb128 archive_index · leb128 offset · leb128 size · leb128 fnv_hash
 */

error_reporting(E_ALL);
ini_set('memory_limit', '2G'); // archives inflate large; bump if you hit a limit

const MASK32 = 0xFFFFFFFF;

/* ----------------------------------------------------------------------------
 * Trove binary primitives (ported 1:1 from troveio.py / archive.py)
 * ------------------------------------------------------------------------- */

/** Read one unsigned LEB128 integer. Returns [value, new_pos]. Masked to 32 bits. */
function read_leb128(string $buf, int $pos): array
{
    $result = 0;
    $shift = 0;
    $len = strlen($buf);
    while (true) {
        if ($pos >= $len) {
            throw new RuntimeException("truncated LEB128 at offset $pos");
        }
        $byte = ord($buf[$pos]);
        $pos++;
        $result |= ($byte & 0x7F) << $shift;
        if (($byte & 0x80) === 0) {
            return [$result & MASK32, $pos];
        }
        $shift += 7;
        if ($shift >= 64) {
            throw new RuntimeException('LEB128 varint too long');
        }
    }
}

/** A signed `char` widened to uint32 (sign extension), matching trove.dll. */
function se(int $b): int
{
    return $b < 0x80 ? $b : ($b | 0xFFFFFF00);
}

/**
 * Trove's FNV-1a-variant checksum (trove.dll). 32-bit unsigned.
 * Full 4-byte words fold little-endian (unsigned); the trailing 1-3 bytes fold
 * big-endian AND sign-extended. Used here only to verify extracted files.
 */
function trove_hash(string $data): int
{
    $FNV_OFFSET = 2166136261;
    $FNV_PRIME  = 16777619;
    $h = $FNV_OFFSET;
    $n = strlen($data);
    $full = $n & ~3;
    for ($i = 0; $i < $full; $i += 4) {
        $chunk = unpack('V', substr($data, $i, 4))[1]; // little-endian uint32
        $h = ($FNV_PRIME * ($h ^ $chunk)) & MASK32;
    }
    $rem = $n & 3;
    if ($rem === 0) {
        return $h & MASK32;
    } elseif ($rem === 1) {
        $val = se(ord($data[$full]));
    } elseif ($rem === 2) {
        $val = ((se(ord($data[$full])) << 8) & MASK32) | se(ord($data[$full + 1]));
    } else { // rem === 3
        $v1  = (se(ord($data[$full])) << 8) & MASK32;
        $v1  = ((se(ord($data[$full + 1])) | $v1) << 8) & MASK32;
        $val = $v1 | se(ord($data[$full + 2]));
    }
    return ($FNV_PRIME * ($h ^ $val)) & MASK32;
}

/** Parse .tfi index bytes into a list of entries (associative arrays). */
function parse_tfi(string $data): array
{
    $entries = [];
    $pos = 0;
    $n = strlen($data);
    while ($pos < $n) {
        [$name_len, $pos] = read_leb128($data, $pos);
        $raw = substr($data, $pos, $name_len);
        $pos += $name_len;
        // Names are UTF-8 and may be null-padded; keep the prefix and posix it.
        $nul = strpos($raw, "\x00");
        $name = ($nul === false) ? $raw : substr($raw, 0, $nul);
        $name = str_replace('\\', '/', $name);

        [$archive_index, $pos] = read_leb128($data, $pos);
        [$offset, $pos]        = read_leb128($data, $pos);
        [$size, $pos]          = read_leb128($data, $pos);
        [$fnv_hash, $pos]      = read_leb128($data, $pos);

        $entries[] = [
            'name'          => $name,
            'archive_index' => $archive_index,
            'offset'        => $offset,
            'size'          => $size,
            'fnv_hash'      => $fnv_hash,
        ];
    }
    return $entries;
}

/** Inflate a .tfa's zlib stream into the concatenated file content. */
function decompress_tfa(string $raw): string
{
    // .tfa is zlib-wrapped (RFC 1950). gzuncompress handles that; fall back to
    // raw deflate just in case a file was stored headerless.
    $out = @gzuncompress($raw);
    if ($out === false) {
        $out = @gzinflate($raw);
    }
    if ($out === false) {
        throw new RuntimeException('could not inflate .tfa (not a valid zlib/deflate stream)');
    }
    return $out;
}

/* ----------------------------------------------------------------------------
 * CLI
 * ------------------------------------------------------------------------- */

function fail(string $msg): void
{
    fwrite(STDERR, "error: $msg\n");
    exit(1);
}

function usage(): void
{
    fwrite(STDERR,
        "Usage: php tfa_extract.php <index.tfi> <archiveN.tfa> [output_dir] [--archive=N] [--list]\n");
    exit(2);
}

/** Make an entry name safe to write under a base dir (no traversal, no abs path). */
function safe_path(string $base, string $name): string
{
    $name = str_replace('\\', '/', $name);
    $parts = [];
    foreach (explode('/', $name) as $part) {
        if ($part === '' || $part === '.' || $part === '..') {
            continue; // drop empty/./.. segments
        }
        $parts[] = $part;
    }
    if (!$parts) {
        $parts = ['unnamed_' . substr(md5($name), 0, 8)];
    }
    return rtrim($base, '/\\') . DIRECTORY_SEPARATOR . implode(DIRECTORY_SEPARATOR, $parts);
}

// --- parse arguments ---------------------------------------------------------
$positional = [];
$forceArchive = null;
$listOnly = false;

foreach (array_slice($argv, 1) as $arg) {
    if ($arg === '--list') {
        $listOnly = true;
    } elseif (strncmp($arg, '--archive=', 10) === 0) {
        $forceArchive = (int) substr($arg, 10);
    } elseif ($arg === '-h' || $arg === '--help') {
        usage();
    } elseif (strncmp($arg, '--', 2) === 0) {
        fail("unknown option: $arg");
    } else {
        $positional[] = $arg;
    }
}

if (count($positional) < 2) {
    usage();
}

$tfiPath = $positional[0];
$tfaPath = $positional[1];
$outDir  = $positional[2] ?? 'extracted';

if (!is_file($tfiPath)) {
    fail("TFI not found: $tfiPath");
}
if (!is_file($tfaPath)) {
    fail("TFA not found: $tfaPath");
}

// --- read + parse the index --------------------------------------------------
$tfiBytes = file_get_contents($tfiPath);
if ($tfiBytes === false) {
    fail("could not read $tfiPath");
}

try {
    $entries = parse_tfi($tfiBytes);
} catch (Throwable $e) {
    fail('malformed .tfi: ' . $e->getMessage());
}

if (!$entries) {
    fail('the .tfi contained no entries');
}

// Group entries by archive index and compute each archive's content span
// (the largest offset+size — i.e. how long that archive inflates to).
$byArchive = [];
$spans = [];
foreach ($entries as $e) {
    $ai = $e['archive_index'];
    $byArchive[$ai][] = $e;
    $end = $e['offset'] + $e['size'];
    if (!isset($spans[$ai]) || $end > $spans[$ai]) {
        $spans[$ai] = $end;
    }
}
ksort($spans);

if ($listOnly) {
    echo "Index: $tfiPath\n";
    echo count($entries) . " file(s) across " . count($spans) . " archive(s):\n\n";
    foreach ($spans as $ai => $span) {
        printf("  archive%d.tfa — %d file(s), %s decompressed\n",
            $ai, count($byArchive[$ai]), number_format($span));
    }
    echo "\nFiles:\n";
    foreach ($entries as $e) {
        printf("  [a%d] %10s  %s\n", $e['archive_index'], number_format($e['size']), $e['name']);
    }
    exit(0);
}

// --- inflate the supplied .tfa -----------------------------------------------
$tfaRaw = file_get_contents($tfaPath);
if ($tfaRaw === false) {
    fail("could not read $tfaPath");
}

try {
    $content = decompress_tfa($tfaRaw);
} catch (Throwable $e) {
    fail($e->getMessage());
}
$contentLen = strlen($content);

// --- decide which archive index this .tfa is ---------------------------------
$archiveIndex = null;

if ($forceArchive !== null) {
    if (!isset($byArchive[$forceArchive])) {
        fail("--archive=$forceArchive has no entries in this .tfi (have: "
            . implode(', ', array_keys($spans)) . ')');
    }
    $archiveIndex = $forceArchive;
} else {
    // Prefer the index whose content span exactly equals the inflated length.
    $exact = [];
    foreach ($spans as $ai => $span) {
        if ($span === $contentLen) {
            $exact[] = $ai;
        }
    }
    if (count($exact) === 1) {
        $archiveIndex = $exact[0];
    } elseif (count($spans) === 1) {
        // Only one archive in the whole index — must be this one.
        $archiveIndex = array_key_first($spans);
        if ($spans[$archiveIndex] !== $contentLen) {
            fwrite(STDERR, sprintf(
                "warning: inflated length %s != index span %s; extracting anyway\n",
                number_format($contentLen), number_format($spans[$archiveIndex])));
        }
    } else {
        // Ambiguous: ask the user to pick.
        fwrite(STDERR, "Could not auto-detect which archive this .tfa is (inflated to "
            . number_format($contentLen) . " bytes).\n");
        fwrite(STDERR, "Re-run with --archive=N, where N is one of:\n");
        foreach ($spans as $ai => $span) {
            fwrite(STDERR, sprintf("  --archive=%d  (span %s%s)\n",
                $ai, number_format($span), $span === $contentLen ? '  <-- length matches' : ''));
        }
        exit(1);
    }
}

// --- extract -----------------------------------------------------------------
$members = $byArchive[$archiveIndex];
echo "Archive index: $archiveIndex  (" . count($members) . " file(s))\n";
echo "Output dir:    $outDir\n\n";

$written = 0;
$okHash = 0;
$badHash = 0;
$truncated = 0;

foreach ($members as $e) {
    $start = $e['offset'];
    $size  = $e['size'];

    if ($start + $size > $contentLen) {
        fwrite(STDERR, "warning: '{$e['name']}' runs past end of archive — skipped\n");
        $truncated++;
        continue;
    }

    $data = substr($content, $start, $size);

    // Verify against the size + FNV hash the index recorded.
    $hashOk = (strlen($data) === $size) && (trove_hash($data) === $e['fnv_hash']);
    if ($hashOk) {
        $okHash++;
    } else {
        $badHash++;
        fwrite(STDERR, "warning: hash mismatch for '{$e['name']}'\n");
    }

    $dest = safe_path($outDir, $e['name']);
    $dir  = dirname($dest);
    if (!is_dir($dir) && !mkdir($dir, 0777, true) && !is_dir($dir)) {
        fail("could not create directory: $dir");
    }
    if (file_put_contents($dest, $data) === false) {
        fail("could not write: $dest");
    }
    $written++;
}

echo "\n";
echo "Done. wrote $written file(s)";
echo " — $okHash verified OK";
if ($badHash)  echo ", $badHash hash-mismatch";
if ($truncated) echo ", $truncated skipped (truncated)";
echo ".\n";
