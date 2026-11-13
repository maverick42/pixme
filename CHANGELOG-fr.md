# Journal des modifications

Toutes les modifications notables de PixMe sont documentées dans ce
fichier, mis à jour à chaque bump de version — en anglais, français et
espagnol (`CHANGELOG.md`, `CHANGELOG-fr.md`, `CHANGELOG-es.md`).

## [0.3.0] - 2026-11-13

### Ajouté

- Espaces de travail : les captures sont rangées par espace de travail
  — chacun possède un nom (30 caractères max), un masque + compteur de
  nommage et un dossier de sortie. Une pastille compacte au début de
  la barre d'état affiche et change l'espace actif ; les captures se
  déplacent entre espaces via le sous-menu « Espace de travail » du
  menu contextuel de l'onglet. Chaque espace mémorise sa dernière
  capture sélectionnée, et « Tout enregistrer » / l'enregistrement à
  la fermeture agissent sur l'ensemble visible.
- Sélecteur d'espace : chaque entrée affiche son nombre de captures
  dans une pastille — teintée accent sur l'espace actif, creuse quand
  l'espace ne contient aucune capture.
- Dupliquer une annotation : `Ctrl+D` / `Cmd+D`, un bouton de la barre
  d'outils juste à droite de Imprimer (grisé sans sélection) et une
  entrée du menu contextuel clonent l'annotation sélectionnée avec un
  petit décalage ; les badges d'étape dupliqués prennent le compteur
  suivant de leur propre mode, et une seule entrée d'annulation
  supprime la copie en restaurant la sélection précédente.
- Numérotation des étapes : les séquences chiffres et lettres sont
  désormais totalement autonomes — dupliquer ou changer le mode d'une
  étape sélectionnée la renumérote dans la séquence cible, et le
  compteur de lettres accepte la saisie d'une lettre.

### Corrigé

- La confirmation « modifications non enregistrées » à la fermeture ne
  bloque plus sur les collisions de noms dans un même espace et
  enregistre les captures de tous les espaces.
- La confirmation de session non enregistrée apparaît désormais à la
  fermeture via le Dock sur macOS.

## [0.2.1] - 2026-11-09

### Ajouté

- Raccourcis globaux configurables, plus captures par framebuffer sur
  macOS.
- Script de packaging macOS produisant le DMG `PixMe.app`.
- Windows : relancer l'application affiche le viewer au lieu d'une
  seconde instance.
- Éditeur Draw : les compteurs d'étape sont initialisés depuis le
  document à l'ouverture ; la banque d'options de chaque outil peut
  être réinitialisée aux valeurs d'usine depuis le menu contextuel de
  sa puce.
- Viewer : taille minimale de fenêtre portée à 580×420 ; cliquer sur
  l'icône de barre des tâches du moniteur affiché recentre le viewer.

### Corrigé

- Les moniteurs déconnectés conservent leur place dans l'ordre de la
  barre d'état au lieu d'être supprimés.
- Le store de session garde sa connexion quand un VACUUM au démarrage
  est ignoré.
- La capture attend la stabilisation du compositeur avant le gel, et
  les sélections de zone se confirment au relâchement pour les
  glisser-créer/placer.

## [0.2.0] - 2026-11-06

Version initiale — capture d'écran et annotation.

### Ajouté

- Capture : écran entier, sélection de zone sur image figée avec
  loupe et poignées redimensionnables, choix fenêtre/objet, zone à
  taille fixe, fenêtres flottantes épinglées à l'écran, compte à
  rebours avant capture, import du presse-papiers.
- Éditeur d'annotations : flèches à courbes de Bézier, texte et boîtes
  de texte, formes avec remplissage, contour et ombre, marqueurs
  d'étape numérotés/letrés, surligneur de zone, estompeur et
  pixelliseur, crayon à main levée, sélection de zone au lasso avec
  opérations sur pixels, autocollants emoji et pipette de couleur —
  avec style par outil persisté entre sessions et annulation/rétablissement
  complets.
- Viewer : captures en onglets et en grille mosaïque, barre de capture
  verticale, barre d'état avec boutons par moniteur, session persistée
  entre redémarrages, enregistrement/export (PNG, JPEG, WebP),
  impression, copie presse-papiers et glisser-déposer vers les
  applications.
- Application résidente : élément de statut macOS avec menus natifs,
  icône de zone de notification Windows, garde d'instance unique.
- Paramètres : interface anglais, français et espagnol, thèmes Pro et
  Pro Light, gestion des moniteurs, styles d'icônes et raccourcis par
  système.
