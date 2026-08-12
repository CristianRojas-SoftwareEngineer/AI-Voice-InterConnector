/* arpa/inet.h stub - compat POSIX para MinGW-w64/UCRT64.
 * El motor incluye <arpa/inet.h> para inet_ntop/inet_pton (ya en ws2tcpip.h).
 * El orden correcto de includes de winsock: windows.h -> winsock2.h -> ws2tcpip.h.
 * sys/socket.h (nuestro shim) ya incluye winsock2.h + ws2tcpip.h. */
#ifndef INGOT_ARPA_INET_SHIM_H
#define INGOT_ARPA_INET_SHIM_H

#include <winsock2.h>
#include <ws2tcpip.h>

/* inet_ntop, inet_pton, inet_addr ya en ws2tcpip.h / winsock2.h. */

#endif /* INGOT_ARPA_INET_SHIM_H */
