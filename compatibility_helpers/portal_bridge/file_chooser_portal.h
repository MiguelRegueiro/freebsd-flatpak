#ifndef FILE_CHOOSER_PORTAL_H
#define FILE_CHOOSER_PORTAL_H
#include "portal_bridge_process.h"
char *rewrite_file_uri(BridgeState *, const char *, bool);
void handle_filechooser_open(BridgeState *, const char *, GVariant *,
                             GDBusMethodInvocation *);
GVariant *rewrite_filechooser_results(BridgeState *, guint32, GVariant *,
                                      bool);
GVariant *rewrite_filechooser_parameters(BridgeState *, GVariant *);
#endif
