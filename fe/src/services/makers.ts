import type { MakerPeriod, Position } from "@/data/makers";
import { skipToken, useInfiniteQuery, useQuery } from "@tanstack/react-query";
import { isMissingRecord, LIVE_QUERY_OPTIONS } from "./live";
import { useServices } from "./context";

export function useMakers() {
  const { makers } = useServices();
  return useQuery({
    ...LIVE_QUERY_OPTIONS,
    queryKey: ["makers"],
    queryFn: () => makers.list(),
  });
}

function useMakerRead<T>(
  name: string,
  id: string | undefined,
  read: (id: string) => Promise<T>,
  period?: MakerPeriod,
) {
  return useQuery({
    ...LIVE_QUERY_OPTIONS,
    queryKey: ["makers", name, id?.toLowerCase(), period],
    queryFn: id ? () => read(id) : skipToken,
    retry: (count, error) => !isMissingRecord(error) && count < 1,
    refetchInterval: (query) =>
      isMissingRecord(query.state.error)
        ? false
        : LIVE_QUERY_OPTIONS.refetchInterval,
    refetchOnWindowFocus: (query) => !isMissingRecord(query.state.error),
    refetchOnReconnect: (query) => !isMissingRecord(query.state.error),
  });
}

export function useMakerDashboard(
  address: string | undefined,
  period: MakerPeriod,
) {
  const { makers } = useServices();
  return useMakerRead(
    "dashboard",
    address,
    (id) => makers.dashboard(id, period),
    period,
  );
}

export function useMakerInventory(
  address: string | undefined,
  period: MakerPeriod,
) {
  const { makers } = useServices();
  return useMakerRead(
    "inventory",
    address,
    (id) => makers.inventory(id, period),
    period,
  );
}

export function useMakerPositions(
  address: string | undefined,
  period: MakerPeriod,
) {
  const { makers } = useServices();
  return useMakerRead(
    "positions",
    address,
    (id) => makers.positions(id, period),
    period,
  );
}

export function usePosition(hash: string | undefined) {
  return useMakerRead("position", hash, useServices().makers.position);
}

export function useMakerSettlements(
  address: string | undefined,
  period: MakerPeriod,
) {
  const { makers } = useServices();
  return useInfiniteQuery({
    ...LIVE_QUERY_OPTIONS,
    queryKey: ["makers", "settlements", address?.toLowerCase(), period],
    initialPageParam: undefined as string | undefined,
    queryFn: address
      ? ({ pageParam }) => makers.settlements(address, period, pageParam)
      : skipToken,
    getNextPageParam: (page) => page.nextCursor,
  });
}

export function usePositionHistory(hash: string | undefined) {
  return useMakerRead("history", hash, useServices().makers.history);
}

export function usePositionDepth(position: Position | undefined) {
  const { makers } = useServices();
  return useQuery({
    ...LIVE_QUERY_OPTIONS,
    queryKey: ["makers", "depth", position?.hash],
    queryFn: position
      ? () => makers.depth(position.hash, position.ref)
      : skipToken,
  });
}
