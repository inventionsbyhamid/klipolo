"""Error responses in one shape: {"error": "<code>", "message": "<plain sentence>"}."""

from fastapi import FastAPI, Request
from fastapi.exceptions import RequestValidationError
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


async def _validation_error(request: Request, exc: RequestValidationError) -> JSONResponse:
    fields: list[str] = []
    for error in exc.errors():
        name = next((part for part in error.get("loc", ())[1:] if isinstance(part, str)), None)
        if name and name not in fields:
            fields.append(name)
    message = f"Check these fields: {', '.join(fields)}." if fields else "The request is not valid."
    return error_response(422, "invalid_request", message)


async def _server_error(request: Request, exc: Exception) -> JSONResponse:
    # Starlette re-raises the exception after this response, so the server
    # still logs the traceback; the client only sees the plain message.
    return error_response(500, "server_error", "Something went wrong. Try again.")


def install_error_handlers(app: FastAPI) -> None:
    app.add_exception_handler(Exception, _server_error)
    app.add_exception_handler(RelayError, _relay_error)
    app.add_exception_handler(StarletteHTTPException, _http_error)
    app.add_exception_handler(RequestValidationError, _validation_error)
