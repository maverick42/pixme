# Registro de cambios

Todos los cambios notables de PixMe se documentan en este archivo, que
se actualiza en cada bump de versión — en inglés, francés y español
(`CHANGELOG.md`, `CHANGELOG-fr.md`, `CHANGELOG-es.md`).

## [0.3.1] - 2026-11-19

### Añadido

- Resaltador a mano alzada: un modo «Resaltador (a mano alzada)» se
  une al resaltador de zona, con un estabilizador que suaviza el
  trazo.
- Cuentagotas a pantalla completa: una selección de color armada
  sigue funcionando cuando el cursor sale de la ventana Draw — una
  superposición por monitor muestra la lupa sobre el escritorio en
  vivo y muestrea el color real de la pantalla; `C` aplica y copia
  `#RRGGBB` sin terminar la selección.
- Selector de color: cada ventana emergente de muestra de color
  muestra ahora los 48 colores base en una cuadrícula fija, más un
  banco persistido de 16 colores personalizados (añadir el color
  actual, clic derecho para vaciar un espacio), junto al selector
  HSV.
- Enderezar: los tiradores interiores de Bézier se atraen a la cuerda
  al arrastrarlos a unos píxeles de ella, y una entrada «Enderezar»
  del menú contextual de la anotación restaura una línea o la línea
  base de un texto en un paso deshacible.
- El botón OK de Draw copia ahora el render anotado al portapapeles
  cuando la opción «copiar al portapapeles» de la barra de captura
  está activada.
- Tira de pestañas: los botones « » saltan a la primera y a la
  última pestaña cuando las fichas desbordan (los chevrones < >
  siguen desplazando); los cuatro botones tienen tooltips.

### Corregido

- La vista previa del resaltador durante el arrastre muestra ahora el
  verdadero resultado multiply a todas las opacidades — la vista
  previa coincide con el trazo final, y un fino contorno mantiene
  visibles los límites del arrastre sobre píxeles oscuros.
- Windows: ya no parpadea una ventana fantasma cuando la
  superposición de captura recupera el foco.
- Los toasts aparecen ahora solo en la ventana que los generó.

## [0.3.0] - 2026-11-13

### Añadido

- Espacios de trabajo: las capturas se agrupan por espacio de trabajo —
  cada uno con su nombre (máx. 30 caracteres), su máscara + contador
  de nomenclatura y su carpeta de salida. Una pastilla compacta al
  inicio de la barra de estado muestra y cambia el espacio activo; las
  capturas se mueven entre espacios desde el submenú «Espacio de
  trabajo» del menú contextual de la pestaña. Cada espacio recuerda su
  última captura seleccionada, y «Guardar todo» / el guardado al
  cerrar actúan sobre el conjunto visible.
- Selector de espacio: cada entrada muestra su número de capturas en
  una pastilla — teñida de acento en el espacio activo, vacía cuando
  el espacio no contiene capturas.
- Duplicar anotación: `Ctrl+D` / `Cmd+D`, un botón de la barra de
  herramientas junto a Imprimir (atenuado sin selección) y una entrada
  del menú contextual clonan la anotación seleccionada con un pequeño
  desplazamiento; los distintivos de paso duplicados toman el contador
  siguiente de su propio modo, y una sola entrada de deshacer elimina
  la copia restaurando la selección anterior.
- Numeración de pasos: las secuencias de números y letras son ahora
  totalmente autónomas — duplicar o cambiar el modo de un paso
  seleccionado lo renumera en la secuencia de destino, y el contador de
  letras acepta letras tecleadas.

### Corregido

- La confirmación de «cambios sin guardar» al cerrar ya no se bloquea
  con colisiones de nombres dentro del mismo espacio y guarda las
  capturas de todos los espacios.
- La confirmación de sesión sin guardar aparece ahora al cerrar desde
  el Dock en macOS.

## [0.2.1] - 2026-11-09

### Añadido

- Atajos globales configurables, además de capturas por framebuffer en
  macOS.
- Script de empaquetado de macOS que produce el DMG `PixMe.app`.
- Windows: relanzar la aplicación muestra el visor en lugar de una
  segunda instancia.
- Editor Draw: los contadores de paso se inicializan desde el documento
  al abrir el editor; el banco de opciones de cada herramienta puede
  restablecerse a los valores de fábrica desde el menú contextual de su
  ficha.
- Visor: tamaño mínimo de ventana ampliado a 580×420; hacer clic en el
  icono de la barra de tareas del monitor mostrado recentra el visor.

### Corregido

- Los monitores desconectados conservan su lugar en el orden de la
  barra de estado en lugar de desaparecer.
- El almacén de sesión mantiene su conexión cuando se omite un VACUUM
  al inicio.
- La captura espera a que el compositor se estabilice antes de
  congelar, y las selecciones de zona se confirman al soltar para los
  arrastres de crear/colocar.

## [0.2.0] - 2026-11-06

Versión inicial — captura de pantalla y anotación.

### Añadido

- Captura: pantalla completa, selección de zona sobre imagen
  congelada con lupa y asas redimensionables, elección de
  ventana/objeto, zona de tamaño fijo, ventanas flotantes fijadas a
  pantalla, cuenta atrás antes de la captura, importación del
  portapapeles.
- Editor de anotaciones: flechas con curvas de Bézier, texto y cajas
  de texto, formas con relleno, contorno y sombra, marcadores de paso
  numerados/letrados, resaltador de zona, difuminador y pixelador,
  lápiz de trazo libre, selección de zona con operaciones sobre
  píxeles, pegatinas emoji y cuentagotas de color — con estilo por
  herramienta persistido entre sesiones y deshacer/rehacer completo.
- Visor: capturas en pestañas y en cuadrícula mosaico, barra de
  captura vertical, barra de estado con botones por monitor, sesión
  persistida entre reinicios, guardar/exportar (PNG, JPEG, WebP),
  impresión, copia al portapapeles y arrastre hacia otras
  aplicaciones.
- Aplicación residente: elemento de estado de macOS con menús
  nativos, icono de bandeja en Windows, guarda de instancia única.
- Ajustes: interfaz en inglés, francés y español, temas Pro y Pro
  Light, gestión de monitores, estilos de iconos y atajos por sistema.
