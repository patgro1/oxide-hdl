import * as fs from "fs";
import * as path from "path";
import { ExtensionContext, window, workspace } from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
} from "vscode-languageclient/node";

let client: LanguageClient | undefined;

export function activate(context: ExtensionContext): void {
  const binary = process.platform === "win32" ? "oxide-hdl.exe" : "oxide-hdl";
  const serverPath = context.asAbsolutePath(path.join("bin", binary));

  if (!fs.existsSync(serverPath)) {
    void window.showErrorMessage(
      `Oxide HDL: server binary not found at ${serverPath}. ` +
        `Try reinstalling the extension.`
    );
    return;
  }

  const serverOptions: ServerOptions = { command: serverPath };

  const clientOptions: LanguageClientOptions = {
    documentSelector: [{ scheme: "file", language: "vhdl" }],
    synchronize: {
      fileEvents: workspace.createFileSystemWatcher("**/*.{vhd,vhdl}"),
    },
  };

  client = new LanguageClient(
    "oxide-hdl",
    "Oxide HDL",
    serverOptions,
    clientOptions
  );

  client.start();
  context.subscriptions.push(client);
}

export function deactivate(): Thenable<void> | undefined {
  return client?.stop();
}
