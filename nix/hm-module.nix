# Home Manager module: `programs.ond`. Installs Ond (on Linux; on macOS it
# ships as an app bundle) and writes the settings you give it to
# managed.json, beside Ond's own settings file. Ond applies them over the
# settings made in the app and locks their controls ("Set in your Nix
# config"); everything you leave out stays the app's to change.
{ self }:
{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.programs.ond;
  json = pkgs.formats.json { };
  inherit (pkgs.stdenv.hostPlatform) isDarwin system;
  managed = json.generate "ond-managed.json" cfg.settings;
in
{
  options.programs.ond = {
    enable = lib.mkEnableOption "Ond, a music player for Navidrome";

    package = lib.mkOption {
      type = lib.types.nullOr lib.types.package;
      default = self.packages.${system}.ond or null;
      defaultText = lib.literalMD "Ond from this flake on Linux, `null` on macOS";
      description = ''
        The package to install, or `null` to install none (on macOS, where
        Ond ships as an app bundle, the settings still apply).
      '';
    };

    settings = lib.mkOption {
      inherit (json) type;
      default = { };
      example = lib.literalExpression ''
        {
          server = "https://music.example.com";
          username = "me";
          theme = "dark";
          accent = "teal";
          playback = {
            replay_gain = "album";
            quality = "auto";
          };
          offline_dir = "/data/music";
        }
      '';
      description = ''
        Settings for Ond, with the names and values of its settings file.
        Each one set here can't be changed in the app; the rest can. Remove
        one and the app goes back to the user's own choice.

        The server and username fill in the sign-in form (the password is
        still asked for, and kept in the system keyring). A downloads folder
        gets a folder of Ond's own inside it, and downloads move there the
        next time Ond starts. docs/home-manager.md lists every setting.
      '';
    };
  };

  config = lib.mkIf cfg.enable (
    lib.mkMerge [
      { home.packages = lib.optional (cfg.package != null) cfg.package; }
      (lib.mkIf (cfg.settings != { } && !isDarwin) {
        xdg.configFile."ond/managed.json".source = managed;
      })
      (lib.mkIf (cfg.settings != { } && isDarwin) {
        home.file."Library/Application Support/dev.solidsilver.ond/managed.json".source = managed;
      })
    ]
  );
}
