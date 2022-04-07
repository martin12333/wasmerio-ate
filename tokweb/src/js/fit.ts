/**
 * Copyright (c) 2017 The xterm.js authors. All rights reserved.
 * @license MIT
 */

import { Terminal } from "xterm";

const MINIMUM_COLS = 2;
const MINIMUM_ROWS = 1;

// The purpose of this file is two-fold:
// First: Expose a function to start a web worker. This function must
// not be inlined into the Rust lib, as otherwise bundlers could not
// bundle it -- huh.
export function termFit(terminal, front) {
  if (!terminal) {
    return undefined;
  }

  if (!terminal.element || !terminal.element.parentElement) {
    return undefined;
  }

  // TODO: Remove reliance on private API
  //const core = (terminal as any)._core;
  let core = terminal._core;

  if (
    core._renderService.dimensions.actualCellWidth === 0 ||
    core._renderService.dimensions.actualCellHeight === 0
  ) {
    return undefined;
  }

  const parentElementStyle = window.getComputedStyle(
    terminal.element.parentElement
  );
  const parentElementHeight =
    parseInt(parentElementStyle.getPropertyValue("height")) || 0;
  const parentElementWidth =
    Math.max(0, parseInt(parentElementStyle.getPropertyValue("width"))) || 0;

  // var parentElementHeight = document.body.clientHeight - 10;
  // var parentElementWidth = document.body.clientWidth - 10;

  const elementStyle = window.getComputedStyle(terminal.element);
  const elementPadding = {
    top: parseInt(elementStyle.getPropertyValue("padding-top")) || 0,
    bottom: parseInt(elementStyle.getPropertyValue("padding-bottom")) || 0,
    right: parseInt(elementStyle.getPropertyValue("padding-right")) || 0,
    left: parseInt(elementStyle.getPropertyValue("padding-left")) || 0,
  };
  const elementPaddingVer = elementPadding.top + elementPadding.bottom;
  const elementPaddingHor = elementPadding.right + elementPadding.left;
  const availableHeight = parentElementHeight - elementPaddingVer;
  const availableWidth =
    parentElementWidth - elementPaddingHor - core.viewport.scrollBarWidth;
  const dims = {
    cols: Math.max(
      MINIMUM_COLS,
      Math.floor(
        availableWidth / core._renderService.dimensions.actualCellWidth
      )
    ),
    rows: Math.max(
      MINIMUM_ROWS,
      Math.floor(
        availableHeight / core._renderService.dimensions.actualCellHeight
      )
    ),
  };

  // Also update the front buffer
  front.width = availableWidth;
  front.height = availableHeight;
  front.style.width = availableWidth + "px";
  front.style.height = availableHeight + "px";

  // Force a full render
  if (terminal.rows !== dims.rows || terminal.cols !== dims.cols) {
    if (dims.rows !== NaN && dims.cols !== NaN) {
      terminal.resize(dims.cols, dims.rows);
    }
  }
}
