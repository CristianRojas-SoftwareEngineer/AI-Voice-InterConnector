"""
Reconstrucción del texto destino a partir de los segmentos traducidos.

`SentenceSegmenter.segment` produce `list[list[str]]` (párrafos, cada uno una
lista ordenada de segmentos); `SegmentAssembler` es la operación inversa una
vez que cada segmento ya fue traducido: reensambla un único texto que
preserva el orden y los saltos de párrafo originales, porque el consumidor
final es la voz y el ritmo/naturalidad del texto reconstruido importa tanto
como la literalidad de cada segmento aislado.
"""


class SegmentAssembler:
    """Reensambla párrafos de segmentos traducidos en un único texto."""

    def assemble(self, paragraphs: list[list[str]]) -> str:
        """Une los segmentos de cada párrafo con un espacio y los párrafos
        entre sí con una línea en blanco, igual que el separador que usa
        `SentenceSegmenter.segment` para partir el texto de entrada."""
        return "\n\n".join(" ".join(segments) for segments in paragraphs)
