# El recibo estampa la versión del crate y no la del producto

Estado: abierto. Detectado en la E2E completa de v0.24.0 en Windows.

## Conducta observada

Con el binario 0.24.0 instalado, el doctor informa:

```
"install": {"receipt": "valid", "version": "0.1.0", ...}
"version": "0.1.0"
```

mientras `version --json` dice `"version": "0.24.0"`.

## Causa

El constructor del entorno de instalación estampa `env!("CARGO_PKG_VERSION")`,
que es la versión del crate `avi-lifecycle` (0.1.0), y el recibo la guarda
como versión instalada. El binario del producto lleva su propia versión
(0.24.0) pero el constructor no la recibe.

## Conducta esperada

El recibo y el doctor declaran la versión del producto instalado (0.24.0),
que es la que `self update` debe comparar para decidir.

## Alcance

Sin tocar hasta ciclo propio: hoy nada compara contra ese campo en
producción, así que es un dato falso sin efecto operativo. Al corregirlo,
revisar qué lectores del recibo asumen su formato.
