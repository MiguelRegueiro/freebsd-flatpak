#ifndef DOCUMENT_MOUNT_BACKEND_H
#define DOCUMENT_MOUNT_BACKEND_H
#include "portal_bridge_process.h"
bool mount_grant_path(const char *, const char *, bool, GError **);
bool unmount_path(const char *);
#endif
