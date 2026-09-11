import { Route } from "react-router-dom";

import { CreatePoolPage } from "@/views/CreatePoolPage";
import { ExplorerPage } from "@/views/ExplorerPage";
import { HomePage } from "@/views/HomePage";
import { MakersPage } from "@/views/MakersPage";
import { NotFoundPage } from "@/views/NotFoundPage";
import { PoolDetailPage } from "@/views/PoolDetailPage";
import { PoolsPage } from "@/views/PoolsPage";
import { StrategyPage } from "@/views/StrategyPage";
import { SwapPage } from "@/views/SwapPage";
import { TradeDetailPage } from "@/views/TradeDetailPage";

/** The whole route table, so which path resolves to which page can be asserted directly. */
export const routes = (
  <>
    <Route path="/" element={<HomePage />} />
    <Route path="/swap" element={<SwapPage />} />
    <Route path="/pools" element={<PoolsPage />} />
    <Route path="/pools/:pair" element={<PoolDetailPage />} />
    <Route path="/pools/:pair/new" element={<CreatePoolPage />} />
    <Route path="/makers" element={<MakersPage />} />
    <Route path="/makers/:maker" element={<MakersPage />} />
    <Route path="/explorer" element={<ExplorerPage />} />
    <Route
      path="/explorer/strategies/:strategyHash"
      element={<StrategyPage />}
    />
    <Route path="/explorer/trades/:tradeId" element={<TradeDetailPage />} />
    <Route path="*" element={<NotFoundPage />} />
  </>
);
