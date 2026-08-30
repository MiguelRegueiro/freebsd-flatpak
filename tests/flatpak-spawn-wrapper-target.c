#include <stdio.h>
#include <stdlib.h>
#include <string.h>

int main(int argc, char **argv)
{
    const char *preload = getenv("LD_PRELOAD");
    const char *expected = getenv("EXPECTED_PRELOAD");
    const char *bus = getenv("DBUS_SESSION_BUS_ADDRESS");
    const char *expected_bus = getenv("EXPECTED_BUS");

    if (argc != 3 || strcmp(argv[1], "first") != 0 ||
        strcmp(argv[2], "second") != 0) {
        fprintf(stderr, "arguments were not preserved\n");
        return 1;
    }
    if (preload == NULL || expected == NULL || strcmp(preload, expected) != 0) {
        fprintf(stderr, "unexpected LD_PRELOAD: %s\n",
                preload == NULL ? "(null)" : preload);
        return 1;
    }
    if (expected_bus != NULL &&
        (bus == NULL || strcmp(bus, expected_bus) != 0)) {
        fprintf(stderr, "unexpected session bus: %s\n",
                bus == NULL ? "(null)" : bus);
        return 1;
    }
    return 0;
}
