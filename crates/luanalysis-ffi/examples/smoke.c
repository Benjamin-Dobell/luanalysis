/*
 * Checks a Lua file and runs a language-server session through luanalysis.h.
 * Usage: smoke <file.lua>. Exits 0 when the library answered as expected.
 */
#include "luanalysis.h"

#include <stdio.h>
#include <string.h>

static int initialized = 0;
static int shut_down = 0;

static void on_message(void *user_data, const char *message, size_t length) {
    (void)user_data;
    if (strlen(message) != length) return;
    if (strstr(message, "\"id\":1,") && strstr(message, "\"capabilities\"")) initialized = 1;
    if (strstr(message, "\"id\":2,") && strstr(message, "\"result\":null")) shut_down = 1;
}

static void send(luanalysis_server *server, const char *message) {
    if (luanalysis_server_send(server, message, strlen(message)) != 0) {
        fprintf(stderr, "the server stopped before %s\n", message);
    }
}

int main(int argc, char **argv) {
    if (argc != 2) {
        fprintf(stderr, "usage: smoke <file.lua>\n");
        return 2;
    }
    printf("luanalysis %s\n", luanalysis_version());

    const char *arguments[] = {"--lua-version", "5.4", argv[1]};
    char *report = luanalysis_check(arguments, 3);
    if (report == NULL) {
        fprintf(stderr, "luanalysis_check failed\n");
        return 1;
    }
    int reported = strstr(report, "\"formatVersion\":1") != NULL;
    printf("%s\n", report);
    luanalysis_string_free(report);

    luanalysis_server *server = luanalysis_server_start(arguments, 3, on_message, NULL);
    if (server == NULL) {
        fprintf(stderr, "luanalysis_server_start failed\n");
        return 1;
    }
    send(server, "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"capabilities\":{}}}");
    send(server, "{\"jsonrpc\":\"2.0\",\"method\":\"initialized\",\"params\":{}}");
    send(server, "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"shutdown\"}");
    send(server, "{\"jsonrpc\":\"2.0\",\"method\":\"exit\"}");
    int code = luanalysis_server_stop(server);
    printf("report %d, initialized %d, shut down %d, exit code %d\n", reported, initialized, shut_down, code);
    return reported && initialized && shut_down && code == 0 ? 0 : 1;
}
