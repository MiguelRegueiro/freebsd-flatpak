#include "spawn_agent.h"
#include <gio/gio.h>
#include <stdio.h>

int main(int argc, char **argv) {
  if (argc != 2 || !g_dbus_is_name(argv[1]) || g_dbus_is_unique_name(argv[1])) {
    fprintf(stderr, "usage: %s WELL_KNOWN_BUS_NAME (argc=%d, name=%s)\n",
            argv[0], argc, argc > 1 ? argv[1] : "missing");
    return 64;
  }
  return run_spawn_agent(argv[1]);
}
