"""
Códigos de salida del CLI — contrato público congelado.

Un orquestador distingue las causas sin parsear texto.
La tabla completa vive aquí para que ningún módulo del paquete
necesite importar de cli.py y ningún sitio redefina una constante.

Criterio de asignación (clase de causa + siguiente llamada del consumidor):
  0  éxito
  1  error genérico (incluye chequeos fallidos de doctor)
  2  entrada inválida (texto vacío, nombre ilegal, colisión, uso incorrecto)
  3  recurso no encontrado voz o audio
  4  modelo no provisionado (ejecutar 'setup')
  5  daemon inalcanzable o no gestionable
  6  conflicto de estado (recurso ocupado, colisión de voz, puerto daemón en uso)
  7  operación no aplicable al contexto actual (sólo lectura, plataforma no soportada)
  8  precondición de entorno incumplida (credenciales, red, permisos, disco)
  130 interrupción por el usuario (128 + SIGINT)
"""

EXIT_OK = 0
EXIT_ERROR = 1
EXIT_INVALID_INPUT = 2
EXIT_NOT_FOUND = 3
EXIT_MODEL_MISSING = 4
EXIT_DAEMON_UNREACHABLE = 5
EXIT_STATE_CONFLICT = 6
EXIT_NOT_APPLICABLE = 7
EXIT_PRECONDITION_FAILED = 8
EXIT_INTERRUPTED = 130


class CliError(BaseException):
    """Fallo de la CLI. Hereda de BaseException (no Exception) para que
    ningún except Exception de dominio la capture y la degrade a 1.

    Atributos:
        code: código de salida del contrato público (EXIT_*)
        reason: razón breve y programática (p. ej. 'usage_error', 'credentials')
        message: mensaje humano legible (se imprime a stderr)
    """

    def __init__(self, code: int, reason: str, message: str) -> None:
        self.code = code
        self.reason = reason
        self.message = message
        super().__init__(message)
