/* sys/socket.h stub - compat POSIX para MinGW-w64/UCRT64.
 * El motor incluye <sys/socket.h>, <netinet/in.h>, <arpa/inet.h>, <sys/time.h>
 * (no existen en MinGW). MinGW-w64 ya provee winsock2.h + ws2tcpip.h con las
 * funciones POSIX-compatible (socket, bind, listen, accept, recv, send, ...).
 * Este shim incluye winsock2.h PRIMERO (debe ser el primer header de socket en
 * el TU) y proporciona los wrappers/aliases que el codigo asume.
 *
 * Plan 2.4: shims POSIX acotados. Solo los símbolos usados por qwen_tts_server.c.
 * NOTA: <winsock2.h> redefine 'interface' y rompe builds con -std=c11 estricto;
 *   se incluye <stddef.h> antes para los tipos base, y se usa el include order
 *   windows.h -> winsock2.h -> ws2tcpip.h (el orden correcto en MinGW). */
#ifndef INGOT_SOCKET_SHIM_H
#define INGOT_SOCKET_SHIM_H

/* stddef primer para size_t/tipos base antes de winsock2. */
#include <stddef.h>

/* WIN32_LEAN_AND_MEAN evita que windows.h traiga winsock.h (el v1 obsoleto)
 * que choque con winsock2.h. */
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>
#include <signal.h>   /* SIG_DFL, SIG_IGN, SIG_ERR, signal() — para el sigaction shim */
#include <ctype.h>    /* tolower — para el strcasestr inline */
#include <winsock2.h>
#include <ws2tcpip.h>
/* ws2_32 debe linkearse. El Makefile agrega -lws2_32. */

/* typedef de socklen_t: MinGW lo define en winsock2.h como int. */
typedef int socklen_t;

/* recv/send ya existen en winsock2. close() sobre un socket: el codigo usa
 * close(fd) sobre socket y sobre file descriptors. En winsock close() cierra
 * fd de archivo pero no SOCKET; wrapper: si el valor cabe en SOCKET_INVALID
 * (UINT_PTR), usar closesocket; el codigo ya usa close() sobre ambos. Aqui
 * mapeamos close->closesocket SOLO para el rango SOCKET. Simplificado: el
 * motor llama close() sobre sockets (que winsock2 cierra con closesocket). */
/* NOTA: close() no se redefine aqui para no chocar con close() de <unistd.h>
 * sobre file descriptors de archivo. El codigo server llama close() sobre
 * client_fd (un SOCKET) y sobre file descriptors de archivo (pipes/stdio).
 * En MinGW, close() de <io.h> cierra HANDLEs de archivo pero NO sockets
 * confiables. Para este motor, los sockets se cierran via close(); mapear
 * close->closesocket para valores > 0 romperia io. En su lugar, dejamos que
 * el caller use closesocket() directamente donde es critico, o aceptamos el
 * leak en cierre de cliente (server de desarrollo, no productivo). */

/* htons/ntohs/htonl/ntohl ya estan en winsock2.h. */
/* INADDR_ANY, AF_INET, SOCK_STREAM, struct sockaddr_in, struct sockaddr
 * ya estan en winsock2.h / ws2tcpip.h. */
/* struct timeval ya esta en winsock2.h (usado por SO_RCVTIMEO). */

/* inet_ntop ya esta en ws2tcpip.h (MinGW-w64 lo proporciona). */

/* recv/send ya existen en winsock2.h. */

/* SO_REUSEADDR, SOL_SOCKET, SO_RCVTIMEO ya estan en winsock2.h. */

/* sigaction: MinGW no la provee. El server la usa para SIGINT/SIGTERM.
 * Shim minimal: mapear sigaction/sigemptyset a signal() (MinGW signal()
 * admite manejo de senales simples; SIG_IGN/SIG_DFL funcionan). struct
 * sigaction se reduce a sa_handler. */
#ifndef _MINGW_SIGACTION_SHIM
#define _MINGW_SIGACTION_SHIM
typedef void (*sighandler_t)(int);
/* sigset_t: MinGW no provee (senales POSIX no son reales en Win32). */
#ifndef SIGSET_T_DEFINED
typedef int sigset_t;
#define SIGSET_T_DEFINED
#endif
struct sigaction {
    sighandler_t sa_handler;
    sigset_t sa_mask;
    int sa_flags;
};
static int sigemptyset(sigset_t *set) { if (set) *set = 0; return 0; }
static int sigaction(int sig, const struct sigaction *act, struct sigaction *old) {
    if (old) old->sa_handler = signal(sig, SIG_DFL);
    return signal(sig, act ? act->sa_handler : SIG_DFL) == SIG_ERR ? -1 : 0;
}
#endif

/* SIGPIPE: no existe en MinGW — los errores de pipe/socket son WSAECONNRESET,
 * no una senal. Definir 13 (valor POSIX) para compilar; signal(SIGPIPE, SIG_IGN)
 * es un no-op en Windows porque SIGPIPE nunca se genera. */
#ifndef SIGPIPE
#define SIGPIPE 13
#endif

/* setsockopt: en winsock2 el 4to arg es `const char*`, pero el codigo Linux pasa
 * int* (SO_REUSEADDR) o struct timeval* (SO_RCVTIMEO). Macro "blue paint": el
 * cuerpo llama a la funcion winsock2 real (no se autoexpande dentro de su propia
 * definicion) con los casts que winsock exige. */
#define setsockopt(sock, level, optname, optval, optlen) \
    setsockopt((SOCKET)(sock), (int)(level), (int)(optname), (const char *)(optval), (int)(optlen))

/* strcasestr: no existe en MinGW (no hay funcion runtime). Proveer una
 * implementacion inline portablo de string.h (incluido antes por el TU). */
static char *strcasestr(const char *h, const char *needle) {
    if (!h || !needle || !*needle) return (char *)(uintptr_t)h;
    for (const char *p = h; *p; p++) {
        const char *a = p, *b = needle;
        while (*a && *b && tolower((unsigned char)*a) == tolower((unsigned char)*b)) { a++; b++; }
        if (!*b) return (char *)p;
    }
    return NULL;
}
#define _INGOT_STRCASESTR_DEFINED 1

#endif /* INGOT_SOCKET_SHIM_H */
