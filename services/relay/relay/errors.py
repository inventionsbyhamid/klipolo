"""Error responses in one shape: {"error": "<code>", "message": "<plain sentence>"}."""

from fastapi import FastAPI, Request
from fastapi.responses import JSONResponse
from starlette.exceptions import HTTPException as StarletteHTTPException


class RelayError(Exception):
    def __init__(
        self,
        status_code: int,
        error: str,
        message: str,
        headers: dict[str, str] | None = None,
    ) -> None:
        super().__init__(message)
        self.status_code = status_code
        self.error = error
        self.message = message
        self.headers = headers


def error_response(
    status_code: int, error: str, message: str, headers: dict[str, str] | None = None
) -> JSONResponse:
    return JSONResponse(
        {"error": error, "message": message}, status_code=status_code, headers=headers
    )


_HTTP_ERRORS = {
    404: ("not_found", "Not found."),
    405: ("method_not_allowed", "This method is not allowed here."),
}


async def _relay_error(request: Request, exc: RelayError) -> JSONResponse:
    return error_response(exc.status_code, exc.error, exc.message, exc.headers)


async def _http_error(request: Request, exc: StarletteHTTPException) -> JSONResponse:
    error, message = _HTTP_ERRORS.get(exc.status_code, ("http_error", "The request failed."))
    return error_response(exc.status_code, error, message, exc.headers)


def install_error_handlers(app: FastAPI) -> None:
    app.add_exception_handler(RelayError, _relay_error)
    app.add_exception_handler(StarletteHTTPException, _http_error)
