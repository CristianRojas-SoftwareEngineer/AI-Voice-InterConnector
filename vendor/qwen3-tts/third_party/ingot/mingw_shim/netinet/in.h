/* netinet/in.h stub - compat POSIX para MinGW-w64/UCRT64.
 * El motor incluye <netinet/in.h> para struct sockaddr_in, IPPROTO_IP, etc.
 * MinGW-w64 los provee en <winsock2.h>. Este stub incluye winsock2.h para
 * exponer los tipos. Debe incluirse DESPUES de sys/socket.h (que incluye
 * winsock2.h primero — orden critico: windows.h -> winsock2.h -> ws2tcpip). */
#ifndef INGOT_NETINET_IN_SHIM_H
#define INGOT_NETINET_IN_SHIM_H

/* sys/socket.h ya incluyo winsock2.h; aqui re-include idempotente. */
#include <winsock2.h>
#include <ws2tcpip.h>

/* htons/ntohs/htonl/ntohl, INADDR_ANY, sockaddr_in ya definidos en winsock2.h. */

#endif /* INGOT_NETINET_IN_SHIM_H */
