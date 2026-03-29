import { Route, Routes } from "react-router-dom";
import { AppLayout } from "../components/AppLayout";
import { HomePage } from "../pages/HomePage";
import { NotFoundPage } from "../pages/NotFoundPage";
import { RunPocPage } from "../pages/RunPocPage";
import { TerminalPocPage } from "../pages/TerminalPocPage";

export function AppRouter() {
  return (
    <Routes>
      <Route element={<AppLayout />}>
        <Route index element={<HomePage />} />
        <Route path="run-poc" element={<RunPocPage />} />
        <Route path="terminal-poc" element={<TerminalPocPage />} />
        <Route path="*" element={<NotFoundPage />} />
      </Route>
    </Routes>
  );
}
