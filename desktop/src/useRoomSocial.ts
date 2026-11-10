import { useEffect, useState } from 'react';
import { listenForRoomSocial, requestRoomSocial } from './bridge';
import type { CoreSnapshot } from './types';
import { RoomSocialController } from './roomSocialController';

export function useRoomSocial(snapshot: CoreSnapshot | null, channelId: string) {
  const key = snapshot ? JSON.stringify([snapshot.serverAddress, snapshot.announcementScope, snapshot.selfSessionId, channelId, snapshot.connection, snapshot.roomSocialSupported, snapshot.roomImagesSupported, snapshot.roomSocialEpoch]) : '';
  const [state, setState] = useState<{ key: string; controller: RoomSocialController } | null>(null);
  const [, render] = useState(0);
  useEffect(() => {
    if (!snapshot || !channelId || channelId === '0' || snapshot.connection !== 'connected') { setState(null); return; }
    const controller = new RoomSocialController({ serverAddress: snapshot.serverAddress, serverFingerprint: snapshot.announcementScope, selfSessionId: snapshot.selfSessionId, channelId }, !!snapshot.roomImagesSupported, !!snapshot.roomSocialSupported, requestRoomSocial);
    let canceled = false;
    let stopEvents = () => {};
    const stopView = controller.subscribe(() => render(value => value + 1));
    setState({ key, controller });
    void listenForRoomSocial(event => controller.accept(event)).then(stop => {
      if (canceled) { stop(); return; }
      stopEvents = stop;
      void controller.refresh();
    }).catch(() => {});
    return () => { canceled = true; stopView(); stopEvents(); controller.dispose(); };
  }, [key]);
  const controller = state?.key === key ? state.controller : null;
  return { controller, social: controller?.snapshot() };
}
