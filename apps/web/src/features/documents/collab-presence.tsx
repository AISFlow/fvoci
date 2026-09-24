import { t } from "@fvoci/i18n";
import { Button } from "@/components/ui/button";
import type { CollabPeer } from "./collab-model";

export function CollabPresence({
  peers,
  onJump,
}: {
  peers: CollabPeer[];
  onJump?: (blockId: string) => void;
}) {
  if (peers.length === 0) return null;
  return (
    <ul className="document-page__presence" aria-label={t("presence.count", { n: peers.length })}>
      {peers.map((peer) => (
        <li key={peer.clientId}>
          <Button
            type="button"
            variant="outline"
            size="sm"
            aria-label={t("presence.jump", { name: peer.name })}
            disabled={!peer.blockId || !onJump}
            onClick={() => {
              if (peer.blockId && onJump) onJump(peer.blockId);
            }}
          >
            {peer.self ? t("presence.selfOtherTab") : peer.name}
          </Button>
        </li>
      ))}
    </ul>
  );
}
