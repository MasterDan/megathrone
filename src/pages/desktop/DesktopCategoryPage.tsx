import type { Component } from "solid-js";

import { DesktopPage } from "@/components/layout/desktop/DesktopPage";
import { CategoryPage } from "@/pages/settings/CategoryPage";

/** The rule editor of one routing category (the mobile page) inside the
 *  content zone. */
export const DesktopCategoryPage: Component = () => (
  <DesktopPage>
    <CategoryPage />
  </DesktopPage>
);
