"""Admin CLI for access codes and relay spend.

Run it next to the server, on the live database:

    python -m relay.admin add-code CODE [--note TEXT]
    python -m relay.admin deactivate-code CODE
    python -m relay.admin enable-sam CODE
    python -m relay.admin set-code-cap CODE [--usd DOLLARS] [--devices COUNT]
    python -m relay.admin list-codes
    python -m relay.admin spend

The database path comes from RELAY_DB_PATH, like the server, unless --db is
given. Changes apply from the next request without a restart: the server
reads a code's active flag and limits on every request.
"""

import argparse
import math
import sqlite3
import sys
from collections.abc import Collection, Sequence
from pathlib import Path

from relay import db
from relay.config import Settings, load_settings

# Registration accepts codes up to this length and trims them, so a longer or
# padded code could never be used.
MAX_CODE_LENGTH = 128
LATEST_ROWS = 20
# What set-code-cap takes to remove a limit.
NO_LIMIT = "none"
# An option set-code-cap was not given: the limit stays as it is.
_KEEP = object()


class AdminError(Exception):
    """A plain message for the operator. The CLI exits with status 1."""


def add_code(conn: sqlite3.Connection, args: argparse.Namespace, settings: Settings) -> None:
    code, note = args.code, args.note
    with db.transaction(conn):
        row = conn.execute("SELECT active FROM codes WHERE code = ?", (code,)).fetchone()
        if row is None:
            conn.execute(
                "INSERT INTO codes (code, active, note) VALUES (?, 1, ?)", (code, note or "")
            )
        else:
            conn.execute(
                "UPDATE codes SET active = 1, note = coalesce(?, note) WHERE code = ?",
                (note, code),
            )
    if row is None:
        print(f'Added code "{code}".')
        return
    message = f'Code "{code}" already exists. '
    message += "It was already active." if row["active"] else "Reactivated it."
    if note is not None:
        message += " Updated its note."
    print(message)


def deactivate_code(
    conn: sqlite3.Connection, args: argparse.Namespace, settings: Settings
) -> None:
    code = args.code
    with db.transaction(conn):
        row = conn.execute("SELECT active FROM codes WHERE code = ?", (code,)).fetchone()
        if row is None:
            raise AdminError(
                f'No code "{code}". Codes are case-sensitive. Run list-codes to see them.'
            )
        conn.execute("UPDATE codes SET active = 0 WHERE code = ?", (code,))
        devices = conn.execute("SELECT count(*) FROM devices WHERE code = ?", (code,)).fetchone()[0]
    if not row["active"]:
        print(f'Code "{code}" was already inactive.')
        return
    effect = (
        f"Relay AI is off for its {_plural(devices, 'device')} from the next request."
        if devices
        else "No devices use it."
    )
    print(f'Deactivated code "{code}". New registrations with it get 403. {effect}')


def set_code_cap(conn: sqlite3.Connection, args: argparse.Namespace, settings: Settings) -> None:
    """A code's limits: the most all its devices may spend together (--usd)
    and how many devices may register with it (--devices). `none` removes
    one; an option left out keeps it."""
    code = args.code
    if args.usd is _KEEP and args.devices is _KEEP:
        raise AdminError(f"Give --usd, --devices or both. Use {NO_LIMIT} to remove a limit.")
    with db.transaction(conn):
        row = conn.execute("SELECT cap_usd, max_devices FROM codes WHERE code = ?", (code,)).fetchone()
        if row is None:
            raise AdminError(f'No code "{code}". Codes are case-sensitive. Run list-codes to see them.')
        cap = row["cap_usd"] if args.usd is _KEEP else args.usd
        devices = row["max_devices"] if args.devices is _KEEP else args.devices
        conn.execute("UPDATE codes SET cap_usd = ?, max_devices = ? WHERE code = ?", (cap, devices, code))
        spent, registered = _code_use(conn, code)
    spend_part = f"no spend cap ({_usd(spent)} spent)" if cap is None else f"spend cap ${cap:.2f} ({_usd(spent)} spent)"
    devices_part = (
        f"no device limit ({registered} registered)"
        if devices is None
        else f"at most {_plural(devices, 'device')} ({registered} registered)"
    )
    print(f'Code "{code}": {spend_part}, {devices_part}.')


def list_codes(conn: sqlite3.Connection, args: argparse.Namespace, settings: Settings) -> None:
    # DEVICES counts every device that ever registered with the code, as its device limit does.
    rows = conn.execute(
        """
        SELECT codes.code, codes.active, codes.sam_off, codes.note, codes.cap_usd, codes.max_devices,
            (SELECT count(*) FROM code_devices WHERE code_devices.code = codes.code) AS devices,
            (SELECT coalesce(sum(cost_usd), 0) FROM ledger WHERE ledger.code = codes.code) AS spent
        FROM codes
        ORDER BY codes.code
        """
    ).fetchall()
    if not rows:
        print("No codes yet. Add one with add-code.")
        return
    _print_table(
        ["CODE", "STATUS", "DEVICES", "SPENT", "CAP", "NOTE"],
        [
            [
                row["code"],
                ("active" if row["active"] else "inactive") + (", video off" if row["sam_off"] else ""),
                str(row["devices"]) + ("" if row["max_devices"] is None else f"/{row['max_devices']}"),
                _usd(row["spent"]),
                "-" if row["cap_usd"] is None else f"${row['cap_usd']:.2f}",
                row["note"],
            ]
            for row in rows
        ],
        right={2, 3, 4},
    )


def spend(conn: sqlite3.Connection, args: argparse.Namespace, settings: Settings) -> None:
    conn.execute("BEGIN")  # one snapshot, so the sections agree
    total = conn.execute(
        "SELECT count(*) AS requests, coalesce(sum(cost_usd), 0) AS spent FROM ledger"
    ).fetchone()
    devices = conn.execute(
        """
        SELECT ledger.device_id, devices.code, count(*) AS requests, sum(cost_usd) AS spent
        FROM ledger JOIN devices ON devices.device_id = ledger.device_id
        GROUP BY ledger.device_id
        ORDER BY spent DESC, ledger.device_id
        """
    ).fetchall()
    latest = conn.execute(
        "SELECT at, device_id, endpoint, model, cost_usd FROM ledger ORDER BY id DESC LIMIT ?",
        (LATEST_ROWS,),
    ).fetchall()
    conn.execute("COMMIT")

    print(
        f"Total: {_usd(total['spent'])} across {_plural(total['requests'], 'request')}. "
        f"Caps: ${settings.device_cap_usd:.2f} per device, "
        f"${settings.global_cap_usd:.2f} overall."
    )
    print()
    if not latest:
        print("No requests yet.")
        return
    print("Per device, highest spend first:")
    _print_table(
        ["DEVICE", "CODE", "REQUESTS", "SPENT"],
        [
            [_short(row["device_id"]), row["code"] or "-", str(row["requests"]), _usd(row["spent"])]
            for row in devices
        ],
        right={2, 3},
    )
    print()
    print(f"Latest requests, newest first (up to {LATEST_ROWS}):")
    _print_table(
        ["AT", "DEVICE", "ENDPOINT", "MODEL", "COST"],
        [
            [
                row["at"],
                _short(row["device_id"]),
                row["endpoint"],
                row["model"] or "-",
                _usd(row["cost_usd"]),
            ]
            for row in latest
        ],
        right={4},
    )


def _code_use(conn: sqlite3.Connection, code: str) -> tuple[float, int]:
    """What was charged under `code`, and how many devices ever registered with it."""
    spent = conn.execute("SELECT coalesce(sum(cost_usd), 0) FROM ledger WHERE code = ?", (code,)).fetchone()[0]
    registered = conn.execute("SELECT count(*) FROM code_devices WHERE code = ?", (code,)).fetchone()[0]
    return spent, registered


def main(argv: Sequence[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    try:
        settings = load_settings()
    except ValueError as exc:
        return _fail(str(exc))
    path = Path(args.db) if args.db else settings.db_path
    # Never create a database here: a mistyped path would hide the change
    # from the server.
    if not path.exists():
        return _fail(
            f"No database at {path}. Check RELAY_DB_PATH or --db, "
            "or start the server once to create it."
        )
    try:
        conn = db.connect(path)
        try:
            args.command(conn, args, settings)
        finally:
            conn.close()
    except AdminError as exc:
        return _fail(str(exc))
    except sqlite3.Error as exc:
        return _fail(f"Database error: {exc}")
    return 0


def enable_sam(conn: sqlite3.Connection, args: argparse.Namespace, settings: Settings) -> None:
    """Video tracking back on for a code: the relay turns it off when Meta bills
    more frames than it reserved, which needs a look first."""
    code = args.code
    with db.transaction(conn):
        if conn.execute("SELECT 1 FROM codes WHERE code = ?", (code,)).fetchone() is None:
            raise AdminError(f'No code "{code}". Codes are case-sensitive. Run list-codes to see them.')
        conn.execute("UPDATE codes SET sam_off = 0 WHERE code = ?", (code,))
    print(f'Video tracking is on again for code "{code}".')


def _parser() -> argparse.ArgumentParser:
    db_help = "database file (default: RELAY_DB_PATH, else ./data/relay.db)"
    parser = argparse.ArgumentParser(
        prog="python -m relay.admin", description="Manage relay access codes and see spend."
    )
    parser.add_argument("--db", metavar="PATH", help=db_help)
    # Also accept --db after the command; SUPPRESS keeps an earlier value.
    db_option = argparse.ArgumentParser(add_help=False)
    db_option.add_argument("--db", metavar="PATH", default=argparse.SUPPRESS, help=db_help)
    commands = parser.add_subparsers(metavar="COMMAND", required=True)

    add = commands.add_parser(
        "add-code",
        parents=[db_option],
        help="add a code, or reactivate an existing one",
    )
    add.add_argument("code", type=_code, help="the code, case-sensitive")
    add.add_argument("--note", help="who the code is for (kept if omitted on an existing code)")
    add.set_defaults(command=add_code)

    deactivate = commands.add_parser(
        "deactivate-code",
        parents=[db_option],
        help="stop a code: no new registrations, no relay AI for its devices",
    )
    deactivate.add_argument("code", type=_code, help="the code, case-sensitive")
    deactivate.set_defaults(command=deactivate_code)

    sam = commands.add_parser(
        "enable-sam",
        parents=[db_option],
        help="turn video tracking (SAM) back on for a code the relay turned it off for",
    )
    sam.add_argument("code", type=_code, help="the code, case-sensitive")
    sam.set_defaults(command=enable_sam)

    cap = commands.add_parser(
        "set-code-cap",
        parents=[db_option],
        help="limit what a code's devices may spend together and how many may register with it",
    )
    cap.add_argument("code", type=_code, help="the code, case-sensitive")
    cap.add_argument(
        "--usd",
        type=_dollars,
        default=_KEEP,
        metavar="DOLLARS",
        help=f"most all its devices may spend together, all time ({NO_LIMIT}: no cap)",
    )
    cap.add_argument(
        "--devices",
        type=_count,
        default=_KEEP,
        metavar="COUNT",
        help=f"most devices that may register with it, counting every one that ever did ({NO_LIMIT}: no limit)",
    )
    cap.set_defaults(command=set_code_cap)

    listing = commands.add_parser("list-codes", parents=[db_option], help="list all codes")
    listing.set_defaults(command=list_codes)

    report = commands.add_parser(
        "spend",
        parents=[db_option],
        help=f"total and per device spend, and the last {LATEST_ROWS} requests",
    )
    report.set_defaults(command=spend)
    return parser


def _code(raw: str) -> str:
    code = raw.strip()
    if not code:
        raise argparse.ArgumentTypeError("the code is empty")
    if len(code) > MAX_CODE_LENGTH:
        raise argparse.ArgumentTypeError(f"the code is longer than {MAX_CODE_LENGTH} characters")
    return code


def _dollars(raw: str) -> float | None:
    if raw.strip().lower() == NO_LIMIT:
        return None
    try:
        value = float(raw)
    except ValueError:
        raise argparse.ArgumentTypeError(f"not an amount in dollars: {raw!r}") from None
    if not math.isfinite(value) or value < 0:
        raise argparse.ArgumentTypeError(f"the amount must be 0 or more, got {raw!r}")
    return value


def _count(raw: str) -> int | None:
    if raw.strip().lower() == NO_LIMIT:
        return None
    try:
        value = int(raw)
    except ValueError:
        raise argparse.ArgumentTypeError(f"not a whole number: {raw!r}") from None
    if value < 0:
        raise argparse.ArgumentTypeError(f"the count must be 0 or more, got {raw!r}")
    return value


def _plural(count: int, noun: str) -> str:
    return f"{count} {noun}" if count == 1 else f"{count} {noun}s"


def _usd(amount: float) -> str:
    return f"${amount:.4f}"


def _short(device_id: str) -> str:
    return device_id[:8]


def _print_table(header: list[str], rows: list[list[str]], right: Collection[int] = ()) -> None:
    widths = [max(len(cell) for cell in column) for column in zip(header, *rows)]
    for line in [header, *rows]:
        cells = [
            cell.rjust(width) if index in right else cell.ljust(width)
            for index, (cell, width) in enumerate(zip(line, widths))
        ]
        print("  ".join(cells).rstrip())


def _fail(message: str) -> int:
    print(message, file=sys.stderr)
    return 1


if __name__ == "__main__":
    sys.exit(main())
