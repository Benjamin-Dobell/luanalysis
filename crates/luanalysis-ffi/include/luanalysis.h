/*
 * Luanalysis: Lua type checking and a language server, in the calling process.
 *
 * Link with libluanalysis (luanalysis.dll and luanalysis.dll.lib on Windows). Strings are
 * UTF-8. No function unwinds or aborts into its caller: a failure is reported by its return
 * value.
 */
#ifndef LUANALYSIS_H
#define LUANALYSIS_H

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

#if defined(_WIN32) && !defined(LUANALYSIS_STATIC)
#define LUANALYSIS_API __declspec(dllimport)
#else
#define LUANALYSIS_API
#endif

/* The library's version, as `luanalysis --version` prints it. Static; do not free. */
LUANALYSIS_API const char *luanalysis_version(void);

/*
 * Checks once, as `luanalysis check --format json <arguments>` does, and returns the JSON
 * report (its "exitCode" is the command's). `arguments` are the command's options and paths,
 * for example {"--lua-version", "5.4", "src"}. Returns NULL when an argument is not UTF-8 or
 * checking failed internally. Free the report with luanalysis_string_free.
 */
LUANALYSIS_API char *luanalysis_check(const char *const *arguments, size_t count);

/* Frees a string this library returned. NULL is ignored. */
LUANALYSIS_API void luanalysis_string_free(char *string);

/* A language server running in this process. */
typedef struct luanalysis_server luanalysis_server;

/*
 * Receives one message for the client: `length` bytes of JSON (a JSON-RPC message without its
 * Content-Length header), followed by a NUL. The message is valid only during the call.
 */
typedef void (*luanalysis_message_callback)(void *user_data, const char *message, size_t length);

/*
 * Starts a language server, as `luanalysis lsp <arguments>` does, but without stdio: deliver the
 * client's messages with luanalysis_server_send; each message for the client is passed to
 * `callback`, in order, on one of the server's threads, until luanalysis_server_stop returns.
 * Without paths in `arguments`, the server checks the client's workspace root (from its
 * initialize request). Returns NULL for invalid arguments or a NULL callback.
 */
LUANALYSIS_API luanalysis_server *luanalysis_server_start(
    const char *const *arguments, size_t count, luanalysis_message_callback callback, void *user_data);

/*
 * Delivers one message from the client: `length` bytes of JSON (a JSON-RPC message without its
 * Content-Length header). May be called from any thread. Returns 0, or -1 when the server has
 * stopped or the message is not UTF-8. A message that is not JSON is answered with a parse
 * error.
 */
LUANALYSIS_API int luanalysis_server_send(const luanalysis_server *server, const char *message, size_t length);

/*
 * Ends the client's stream, waits for the server, and frees it. Returns the server's exit code:
 * 0 after the client's shutdown request and exit notification, 1 otherwise. NULL is ignored and
 * returns 1.
 */
LUANALYSIS_API int luanalysis_server_stop(luanalysis_server *server);

#ifdef __cplusplus
}
#endif

#endif
