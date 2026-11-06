// One small inline SVG icon set (24×24 grid, `currentColor`).

import type { SVGProps } from "react";

type IconProps = SVGProps<SVGSVGElement> & { size?: number };

function Icon({ size = 18, children, ...rest }: IconProps) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.8}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
      {...rest}
    >
      {children}
    </svg>
  );
}

export const PlayIcon = (p: IconProps) => (
  <Icon {...p}>
    <path d="M7 4.5v15l12.5-7.5z" fill="currentColor" stroke="none" />
  </Icon>
);
export const PauseIcon = (p: IconProps) => (
  <Icon {...p}>
    <rect x="6" y="4.5" width="4" height="15" rx="1" fill="currentColor" stroke="none" />
    <rect x="14" y="4.5" width="4" height="15" rx="1" fill="currentColor" stroke="none" />
  </Icon>
);
export const PreviousIcon = (p: IconProps) => (
  <Icon {...p}>
    <path d="M18 5v14L8 12z" fill="currentColor" stroke="none" />
    <path d="M6 5v14" strokeWidth={2.2} />
  </Icon>
);
export const NextIcon = (p: IconProps) => (
  <Icon {...p}>
    <path d="M6 5v14l10-7z" fill="currentColor" stroke="none" />
    <path d="M18 5v14" strokeWidth={2.2} />
  </Icon>
);
export const RepeatIcon = (p: IconProps) => (
  <Icon {...p}>
    <path d="M17 2l3 3-3 3" />
    <path d="M4 11V9a4 4 0 0 1 4-4h12" />
    <path d="M7 22l-3-3 3-3" />
    <path d="M20 13v2a4 4 0 0 1-4 4H4" />
  </Icon>
);
export const RepeatOneIcon = (p: IconProps) => (
  <Icon {...p}>
    <path d="M17 2l3 3-3 3" />
    <path d="M4 11V9a4 4 0 0 1 4-4h12" />
    <path d="M7 22l-3-3 3-3" />
    <path d="M20 13v2a4 4 0 0 1-4 4H4" />
    <path d="M11 10l1.5-1v6" strokeWidth={1.6} />
  </Icon>
);
export const VolumeIcon = (p: IconProps) => (
  <Icon {...p}>
    <path d="M4 9v6h4l5 4V5L8 9z" fill="currentColor" stroke="none" />
    <path d="M16 9a4 4 0 0 1 0 6" />
    <path d="M18.5 6.5a7.5 7.5 0 0 1 0 11" />
  </Icon>
);
export const VolumeMutedIcon = (p: IconProps) => (
  <Icon {...p}>
    <path d="M4 9v6h4l5 4V5L8 9z" fill="currentColor" stroke="none" />
    <path d="M16.5 9.5l5 5M21.5 9.5l-5 5" />
  </Icon>
);
export const QueueIcon = (p: IconProps) => (
  <Icon {...p}>
    <path d="M4 6h12M4 12h12M4 18h8" />
    <path d="M17 15v6l4-3z" fill="currentColor" stroke="none" />
  </Icon>
);
export const CloseIcon = (p: IconProps) => (
  <Icon {...p}>
    <path d="M6 6l12 12M18 6L6 18" />
  </Icon>
);
export const SearchIcon = (p: IconProps) => (
  <Icon {...p}>
    <circle cx="11" cy="11" r="6.5" />
    <path d="M20 20l-4.2-4.2" />
  </Icon>
);
export const ArtistIcon = (p: IconProps) => (
  <Icon {...p}>
    <circle cx="12" cy="8" r="4" />
    <path d="M4.5 20a7.5 7.5 0 0 1 15 0" />
  </Icon>
);
export const AlbumIcon = (p: IconProps) => (
  <Icon {...p}>
    <circle cx="12" cy="12" r="8.5" />
    <circle cx="12" cy="12" r="2" />
  </Icon>
);
export const FolderIcon = (p: IconProps) => (
  <Icon {...p}>
    <path d="M3.5 7a2 2 0 0 1 2-2h4l2 2h7a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2h-13a2 2 0 0 1-2-2z" />
  </Icon>
);
export const PlaylistIcon = (p: IconProps) => (
  <Icon {...p}>
    <path d="M4 6h11M4 11h11M4 16h6" />
    <circle cx="17" cy="17" r="2.5" />
    <path d="M19.5 17V8l2-1" />
  </Icon>
);
export const SettingsIcon = (p: IconProps) => (
  <Icon {...p}>
    <circle cx="12" cy="12" r="3" />
    <path d="M12 2.5v3M12 18.5v3M2.5 12h3M18.5 12h3M5.3 5.3l2.1 2.1M16.6 16.6l2.1 2.1M5.3 18.7l2.1-2.1M16.6 7.4l2.1-2.1" />
  </Icon>
);
export const NoteIcon = (p: IconProps) => (
  <Icon {...p}>
    <path d="M9 18V5l11-2v13" />
    <circle cx="6.5" cy="18" r="2.5" />
    <circle cx="17.5" cy="16" r="2.5" />
  </Icon>
);
export const AlertIcon = (p: IconProps) => (
  <Icon {...p}>
    <path d="M12 3.5l9.5 16.5h-19z" />
    <path d="M12 10v4.5M12 17.5v.01" />
  </Icon>
);
export const ShuffleIcon = (p: IconProps) => (
  <Icon {...p}>
    <path d="M3.5 7h3.5c4 0 6 10 10 10h3.5M3.5 17h3.5c1.6 0 2.8-1.6 3.9-3.6M13.1 9.6C14.2 7.6 15.4 7 17 7h3.5" />
    <path d="M18 4.5l2.5 2.5-2.5 2.5M18 14.5l2.5 2.5-2.5 2.5" />
  </Icon>
);
export const MoreIcon = (p: IconProps) => (
  <Icon {...p}>
    <circle cx="5.5" cy="12" r="1.4" fill="currentColor" stroke="none" />
    <circle cx="12" cy="12" r="1.4" fill="currentColor" stroke="none" />
    <circle cx="18.5" cy="12" r="1.4" fill="currentColor" stroke="none" />
  </Icon>
);
export const BackIcon = (p: IconProps) => (
  <Icon {...p}>
    <path d="M15 5l-7 7 7 7" />
  </Icon>
);
/** Save the queue as a playlist: the playlist lines with a plus. */
export const PlaylistAddIcon = (p: IconProps) => (
  <Icon {...p}>
    <path d="M4 6h11M4 11h11M4 16h6" />
    <path d="M17.5 13v8M13.5 17h8" />
  </Icon>
);
/** Clear the queue: a broom. */
export const BroomIcon = (p: IconProps) => (
  <Icon {...p}>
    <path d="M13 11l7.5-7.5" />
    <path d="M11 9l4 4" />
    <path d="M11 9l-6.5 4.5-1 7 7-1L15 13" />
    <path d="M7 16.5l-1.5 3M10 17.5l-.8 1.8" />
  </Icon>
);
export const ForwardIcon = (p: IconProps) => (
  <Icon {...p}>
    <path d="M9 5l7 7-7 7" />
  </Icon>
);
export const HomeIcon = (p: IconProps) => (
  <Icon {...p}>
    <path d="M4 10.5L12 4l8 6.5" />
    <path d="M6.5 9v10.5h11V9" />
    <path d="M10 19.5v-5h4v5" />
  </Icon>
);
/** The local library (this computer). */
export const LocalIcon = (p: IconProps) => (
  <Icon {...p}>
    <rect x="3.5" y="5" width="17" height="11" rx="1.5" />
    <path d="M9 19.5h6M12 16v3.5" />
  </Icon>
);
/** A Navidrome server. */
export const ServerIcon = (p: IconProps) => (
  <Icon {...p}>
    <rect x="4" y="4.5" width="16" height="6" rx="1.5" />
    <rect x="4" y="13.5" width="16" height="6" rx="1.5" />
    <path d="M7.5 7.5h.01M7.5 16.5h.01" strokeWidth={2.6} />
  </Icon>
);
