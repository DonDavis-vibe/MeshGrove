# RPM of a staged tree (packaging/linux/stage.sh), built on the Fedora release it is meant for:
#   rpmbuild -bb --define "_topdir $PWD/rpmbuild" --define "stage $PWD/stage" \
#       --define "pkgversion 0.1.0" packaging/linux/meshgrove.spec
# rpm reads the dependencies (libarchive, OpenCascade, …) from the binaries.

Name:           meshgrove
Version:        %{pkgversion}
Release:        1%{?dist}
Summary:        Gallery for 3D-print files (STL, 3MF, STEP, OBJ, PLY, glTF) with previews
License:        GPL-3.0-or-later
URL:            https://github.com/DonDavis-vibe/MeshGrove
ExclusiveArch:  x86_64
Recommends:     poppler-utils

# The binaries come from cargo, already stripped.
%global debug_package %{nil}

%description
Meshgrove browses folders of 3D-print files like a photo gallery: every STL,
3MF, STEP, OBJ, PLY and glTF model, every model inside ZIP, 7z and RAR
archives and every PDF gets a real preview. It has a 3D viewer with
measuring, a searchable library with tags and notes, a duplicate finder, a
print bed check and hands files over to the slicer. Includes thumbnails for
Dolphin and other file managers.

%install
cp -a %{stage}/. %{buildroot}/
mkdir -p %{buildroot}%{_licensedir}/%{name}
mv %{buildroot}%{_docdir}/%{name}/LICENSE %{buildroot}%{_docdir}/%{name}/THIRD-PARTY-LICENSES.md \
    %{buildroot}%{_licensedir}/%{name}/

%files
%dir %{_docdir}/%{name}
%dir %{_licensedir}/%{name}
%{_bindir}/meshgrove
%{_bindir}/meshgrove-step-mesher
%{_bindir}/meshgrove-thumbnailer
%{_datadir}/applications/meshgrove.desktop
%{_datadir}/icons/hicolor/*/apps/meshgrove.png
%{_datadir}/mime/packages/meshgrove-step.xml
%{_datadir}/thumbnailers/meshgrove.thumbnailer
%doc %{_docdir}/%{name}/README.md
%license %{_licensedir}/%{name}/LICENSE
%license %{_licensedir}/%{name}/THIRD-PARTY-LICENSES.md
