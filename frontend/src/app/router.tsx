import {
  createRootRoute,
  createRoute,
  createRouter,
} from "@tanstack/react-router";

import { ExplorerPage } from "@/views/explorer/ExplorerPage";
import { HomePage } from "@/views/home/HomePage";
import { Layout } from "@/views/layout/Layout";
import { MakersPage } from "@/views/makers/MakersPage";
import { PoolsPage } from "@/views/pools/PoolsPage";
import { SwapPage } from "@/views/swap/SwapPage";

const rootRoute = createRootRoute({ component: Layout });

const routes = [
  createRoute({
    getParentRoute: () => rootRoute,
    path: "/",
    component: HomePage,
  }),
  createRoute({
    getParentRoute: () => rootRoute,
    path: "/swap",
    component: SwapPage,
  }),
  createRoute({
    getParentRoute: () => rootRoute,
    path: "/pools",
    component: PoolsPage,
  }),
  createRoute({
    getParentRoute: () => rootRoute,
    path: "/makers",
    component: MakersPage,
  }),
  createRoute({
    getParentRoute: () => rootRoute,
    path: "/explorer",
    component: ExplorerPage,
  }),
];

const routeTree = rootRoute.addChildren(routes);

export const router = createRouter({ routeTree });

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}
