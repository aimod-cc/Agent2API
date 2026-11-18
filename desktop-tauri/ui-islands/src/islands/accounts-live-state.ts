let connected = false

export const accountEventsConnected = (): boolean => connected
export function setAccountEventsConnected(value: boolean): void { connected = value }
