// OpenCascade glue for stlb-step-mesher: STEP -> tessellated binary STL (millimetres).

#include <BRepBndLib.hxx>
#include <BRepMesh_IncrementalMesh.hxx>
#include <BRepPrimAPI_MakeBox.hxx>
#include <Bnd_Box.hxx>
#include <STEPControl_Reader.hxx>
#include <STEPControl_Writer.hxx>
#include <Standard_Failure.hxx>
#include <StlAPI_Writer.hxx>
#include <TopoDS_Shape.hxx>

#include <cmath>
#include <cstdio>

// Returns 0 on success; other values say which step failed (see main.rs).
extern "C" int stlb_step_to_stl(const char* input, const char* output, double relative_deflection) {
    try {
        STEPControl_Reader reader;
        if (reader.ReadFile(input) != IFSelect_RetDone) return 2;
        if (reader.TransferRoots() == 0) return 3;
        TopoDS_Shape shape = reader.OneShape();
        if (shape.IsNull()) return 3;

        // Chordal tolerance relative to the part size: fine enough for the viewer, cheap for big parts.
        Bnd_Box box;
        BRepBndLib::Add(shape, box);
        if (box.IsVoid()) return 4;
        const double diagonal = std::sqrt(box.SquareExtent());
        BRepMesh_IncrementalMesh mesher(shape, diagonal * relative_deflection, Standard_False, 0.35, Standard_True);
        if (!mesher.IsDone()) return 5;

        StlAPI_Writer writer;
        writer.ASCIIMode() = Standard_False;
        if (!writer.Write(shape, output)) return 6;
        return 0;
    } catch (const Standard_Failure& e) {
        std::fprintf(stderr, "OpenCascade: %s\n", e.GetMessageString());
        return 7;
    } catch (...) {
        return 8;
    }
}

// Writes a 10 mm cube as STEP: lets tests produce a real STEP file without shipping one.
extern "C" int stlb_write_test_cube(const char* output) {
    try {
        TopoDS_Shape cube = BRepPrimAPI_MakeBox(10.0, 10.0, 10.0).Shape();
        STEPControl_Writer writer;
        if (writer.Transfer(cube, STEPControl_AsIs) != IFSelect_RetDone) return 2;
        return writer.Write(output) == IFSelect_RetDone ? 0 : 3;
    } catch (...) {
        return 8;
    }
}
