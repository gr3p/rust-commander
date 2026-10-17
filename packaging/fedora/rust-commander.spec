# CI passes ver. A local build without it uses the base version.
%{!?ver:%global ver 0.1.0}
%global debug_package %{nil}

Name:           rust-commander
Version:        %{ver}
Release:        1%{?dist}
Summary:        Dual-pane file manager
License:        MIT
URL:            https://github.com/gr3p/rust-commander
Source0:        %{name}-%{ver}.tar.gz
BuildRequires:  cargo
BuildRequires:  rust
BuildRequires:  pkgconf-pkg-config
BuildRequires:  gcc
BuildRequires:  gtk4-devel
BuildRequires:  vte291-gtk4-devel
Requires:       hicolor-icon-theme

%description
Dual-pane file manager for Linux, with built-in terminals, viewers and themes.

%prep
%autosetup -n %{name}-%{ver}

%build
cargo build --locked --release

%install
install -Dm755 target/release/%{name} %{buildroot}%{_bindir}/%{name}
install -Dm644 data/rust-commander.desktop \
	%{buildroot}%{_datadir}/applications/se.rustcommander.RustCommander.desktop
install -Dm644 img/rust-commander.png \
	%{buildroot}%{_datadir}/icons/hicolor/512x512/apps/%{name}.png

%files
%license LICENSE
%{_bindir}/%{name}
%{_datadir}/applications/se.rustcommander.RustCommander.desktop
%{_datadir}/icons/hicolor/512x512/apps/%{name}.png

%changelog
* Sat Oct 17 2026 Marcus Wallén <sysopt@protonmail.com> - 0.1.0-1
- Initial package.
