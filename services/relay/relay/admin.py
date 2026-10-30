"""Admin CLI for access codes and relay spend.

Run it next to the server, on the live database:

    python -m relay.admin add-code CODE [--note TEXT]
    python -m relay.admin deactivate-code CODE
    python -m relay.admin list-codes
    python -m relay.admin spend

The database path comes from RELAY_DB_PATH, like the server, unless --db is
given. Changes apply from the next request without a restart: the server
reads a code's active flag on every request.
"""

import argparse
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


def list_codes(conn: sqlite3.Connection, args: argparse.Namespace, settings: Settings) -> None:
    rows = conn.execute(
        """
        SELECT codes.code, codes.active, codes.note, count(devices.device_id) AS devices
        FROM codes LEFT JOIN devices ON devices.code = codes.code
        GROUP BY codes.code
        ORDER BY codes.code
        """
    ).fetchall()
    if not rows:
        print("No codes yet. Add one with add-code.")
        return
    _print_table(
        ["CODE", "STATUS", "DEVICES", "NOTE"],
        [
            [
                row["code"],
                "active" if row["active"] else "inactive",
                str(row["devices"]),
                row["note"],
            ]
            for row in rows
        ],
        right={2},
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
